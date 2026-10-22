//! Owner-host boundary around the actual DLL input-composition ABI.
use super::*;
use sumibi_tip::spike::{ISpikeInput, ISpikeInputMonitor, ISpikeInputSignal, FUNCTION_TEST_INPUT};
use sumibi_tip::spike::{
    ISpikeInputEraser, ISpikeInputWriter, INPUT_APPLIED_UNTRACKED, INPUT_NOT_ATTEMPTED,
    INPUT_UNCERTAIN,
};
use windows::Win32::Foundation::HANDLE;
#[path = "input_notify_host.rs"]
pub(super) mod input_notify_host;
pub(crate) use input_notify_host::exercise_notifications;
pub(super) use input_notify_host::test_notifications;
#[path = "input_transaction_host.rs"]
mod input_transaction_host;
pub(super) use input_transaction_host::test_transactions;
pub(crate) use input_transaction_host::{exercise_transactions, run_transaction_demo};
#[path = "input_backspace_host.rs"]
mod input_backspace_host;
pub(crate) use input_backspace_host::exercise_backspace;
pub(super) use input_backspace_host::test_backspace;

pub(super) struct Composing {
    control: ISpikeInput,
    settled_composition: Cell<u32>,
    monitor: ISpikeInputMonitor,
    signal: ComObject<crate::input_signal::Signal>,
    uncertain: Cell<bool>,
    boundary_pending: Cell<bool>,
    boundary_blocked: Cell<bool>,
    boundary_generation: Rc<Cell<Option<u64>>>,
}
impl Composing {
    pub(super) fn backspace(&self, editor: &Editor, count: u32) -> Result<u32> {
        if self.boundary_blocked.get() {
            self.cancel(editor, false);
        }
        if self.uncertain.get() || self.boundary_blocked.get() {
            return Ok(INPUT_NOT_ATTEMPTED);
        }
        if self.settled_composition.get() != editor.store.get().composition_events.get() {
            self.cancel(editor, false);
        }
        if editor.store.get().composition_events.get() == u32::MAX {
            return Ok(INPUT_NOT_ATTEMPTED);
        }
        let Ok(eraser) = self.control.cast::<ISpikeInputEraser>() else {
            return Ok(INPUT_NOT_ATTEMPTED);
        };
        let mut disposition = INPUT_UNCERTAIN;
        unsafe {
            let _ = eraser.Backspace(editor.context.as_raw(), count, &mut disposition);
        }
        self.accept_disposition(editor, disposition);
        Ok(disposition)
    }
    fn accept_disposition(&self, editor: &Editor, disposition: u32) {
        self.settled_composition
            .set(editor.store.get().composition_events.get());
        if matches!(disposition, INPUT_UNCERTAIN | INPUT_APPLIED_UNTRACKED) {
            self.uncertain.set(true);
        }
    }
    pub(super) fn insert(&self, editor: &Editor, units: &[u16]) -> Result<u32> {
        if self.boundary_blocked.get() {
            self.cancel(editor, false);
        }
        if self.uncertain.get() || self.boundary_blocked.get() {
            return Ok(INPUT_NOT_ATTEMPTED);
        }
        if self.settled_composition.get() != editor.store.get().composition_events.get() {
            self.cancel(editor, false);
        }
        if editor.store.get().composition_events.get() == u32::MAX {
            return Ok(INPUT_NOT_ATTEMPTED);
        }
        let Ok(writer) = self.control.cast::<ISpikeInputWriter>() else {
            return Ok(INPUT_NOT_ATTEMPTED);
        };
        let mut disposition = INPUT_UNCERTAIN;
        unsafe {
            let _ = writer.Insert(
                editor.context.as_raw(),
                units.as_ptr(),
                units.len() as u32,
                &mut disposition,
            );
        }
        self.accept_disposition(editor, disposition);
        Ok(disposition)
    }
    pub(super) fn new(manager: &ITfThreadMgr) -> Result<Self> {
        let control: ISpikeInput = unsafe {
            manager
                .GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)?
                .GetFunction(&FUNCTION_TEST_INPUT, &ISpikeInput::IID)?
                .cast()?
        };
        let monitor: ISpikeInputMonitor = control.cast()?;
        let signal = crate::input_signal::Signal::new()?;
        unsafe {
            monitor
                .SetSignal(signal.to_interface::<ISpikeInputSignal>().as_raw())
                .ok()?;
        }
        Ok(Self {
            control,
            settled_composition: Cell::new(0),
            monitor,
            signal,
            uncertain: Cell::new(false),
            boundary_pending: Cell::new(false),
            boundary_blocked: Cell::new(false),
            boundary_generation: Rc::new(Cell::new(Some(0))),
        })
    }
    pub(super) fn connect(&self, editor: &Editor) {
        *editor.store.get().input_signal.borrow_mut() = Some(self.signal.clone());
    }
    pub(super) fn wait_handle(&self) -> Option<HANDLE> {
        self.signal.get().handle()
    }
    /// The cooperating owner observed a boundary. No COM, text read or edit
    /// here: block reuse now and coalesce cleanup into the existing owner event.
    pub(super) fn owner_boundary(&self) -> Result<()> {
        self.boundary_generation.set(
            self.boundary_generation
                .get()
                .and_then(|epoch| epoch.checked_add(1)),
        );
        self.boundary_blocked.set(true);
        self.boundary_pending.set(true);
        self.signal.get().notify()
    }
    pub(super) fn poll(&self, editor: &Editor) -> Result<bool> {
        if !self.signal.get().drain()? {
            return Ok(false);
        }
        let composition = editor.store.get().composition_events.get();
        let epoch = self.boundary_generation.get();
        let boundary = self.boundary_pending.replace(false);
        let result =
            if boundary || composition == u32::MAX || composition != self.settled_composition.get()
            {
                unsafe { self.control.Cancel(0).ok() }
            } else {
                unsafe { self.monitor.Poll().ok() }
            };
        if boundary && result.is_ok() && epoch.is_some() && self.boundary_generation.get() == epoch
        {
            self.boundary_blocked.set(false);
        }
        self.settled_composition
            .set(editor.store.get().composition_events.get());
        // A denied cleanup stays logically cancelled. Wait for another status
        // notification/user action rather than spinning on a failing lock.
        let _ = result;
        Ok(true)
    }
    pub(super) fn close(&self, editor: &Editor) -> Result<()> {
        self.signal.get().close()?;
        editor.store.get().input_signal.borrow_mut().take();
        unsafe {
            self.monitor.SetSignal(std::ptr::null_mut()).ok()?;
            self.control.Cancel(0).ok()
        }
    }
    pub(super) fn before(&self, editor: &Editor, backspace: bool) -> bool {
        if self.boundary_blocked.get()
            || self.settled_composition.get() != editor.store.get().composition_events.get()
        {
            self.cancel(editor, false);
        }
        if self.boundary_blocked.get() || editor.store.get().composition_events.get() == u32::MAX {
            return false;
        }
        unsafe {
            self.control
                .BeforeInput(editor.context.as_raw(), backspace.into())
                .is_ok()
        }
    }
    pub(super) fn after(&self, editor: &Editor, units: &[u16], recorded: bool) {
        unsafe {
            if recorded {
                let _ = self.control.AfterInput(units.as_ptr(), units.len() as u32);
            } else {
                let _ = self.control.Cancel(0);
            }
        }
        self.settled_composition
            .set(editor.store.get().composition_events.get());
    }
    pub(super) fn cancel(&self, editor: &Editor, user_boundary: bool) {
        if user_boundary {
            self.uncertain.set(false);
            editor.store.get().input_problem.set(false);
        }
        let epoch = self.boundary_generation.get();
        let cancelled = unsafe { self.control.Cancel(user_boundary.into()).is_ok() };
        if cancelled && epoch.is_some() && self.boundary_generation.get() == epoch {
            self.boundary_pending.set(false);
            self.boundary_blocked.set(false);
        }
        self.settled_composition
            .set(editor.store.get().composition_events.get());
    }
    pub(super) fn prepare(&self, editor: &Editor, bridge: &ComObject<Bridge>) -> Result<bool> {
        if self.uncertain.get() || self.boundary_blocked.get() {
            return Ok(false);
        }
        let generation = self.boundary_generation.clone();
        let Some(epoch) = generation.get() else {
            return Ok(false);
        };
        if self.settled_composition.get() != editor.store.get().composition_events.get() {
            self.cancel(editor, false);
            return Ok(false);
        }
        if unsafe { self.control.PrepareConversion().is_err() } {
            return Ok(false);
        }
        if self.boundary_blocked.get() || generation.get() != Some(epoch) {
            return Ok(false);
        }
        let composition = editor.store.get().composition_events.get();
        self.settled_composition.set(composition);
        let control = self.control.clone();
        let store = editor.store.clone();
        bridge.get().check_next_input(Box::new(move |units| {
            if generation.get() != Some(epoch)
                || composition == u32::MAX
                || store.get().composition_events.get() != composition
            {
                return Err(windows::Win32::Foundation::E_ABORT.into());
            }
            unsafe {
                control
                    .CheckSource(units.as_ptr(), units.len() as u32)
                    .ok()?;
            }
            if generation.get() != Some(epoch)
                || store.get().composition_events.get() != composition
            {
                return Err(windows::Win32::Foundation::E_ABORT.into());
            }
            Ok(())
        }))?;
        Ok(true)
    }
}

pub(crate) fn run_composing_demo(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    run(manager, client, RunMode::ComposingDemo)
}
pub(crate) fn exercise_composing(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for case in [
        "grow",
        "shrink",
        "erase",
        "move",
        "external-restored",
        "foreign",
        "forced",
        "private",
        "denied",
        "escape",
        "enter",
        "apply",
        "host-denied",
        "private-input",
        "begin-denied",
        "missing-sink",
        "private-restored",
        "forced-foreign",
        "explicit",
        "focus",
        "readonly",
        "resize-denied",
        "external-new",
        "gate-source",
        "gate-selection",
        "body-gate",
        "pending-edit",
        "empty-cycle",
        "close",
    ] {
        run(manager, client, RunMode::ComposingTest(case))?;
    }
    Ok(())
}
pub(super) fn test_composing(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    let store = editor.store.get();
    store.external_text("prefix  suffix".encode_utf16().collect())?;
    store.host_selection(7, 7)?;
    pump(window)?;
    match case {
        "host-denied" => store.reject_composition.set(true),
        "private-input" => set_scope(editor, IS_PRIVATE)?,
        "begin-denied" => store.deny_lock.set(true),
        "missing-sink" => unsafe {
            editor
                .context
                .cast::<ITfCompartmentMgr>()?
                .ClearCompartment(client, &sumibi_tip::spike::GUID_SPIKE_COMPOSITION_SINK)?;
        },
        _ => {}
    }
    for unit in "a😀b".encode_utf16() {
        editor.input(WM_CHAR, WPARAM(unit.into()), LPARAM(1))?;
    }
    if matches!(
        case,
        "host-denied" | "private-input" | "begin-denied" | "missing-sink"
    ) {
        assert_eq!(
            *store.text.borrow(),
            "prefix a😀b suffix".encode_utf16().collect::<Vec<_>>(),
            "{case}: ordinary input exactly once"
        );
        assert_eq!(store.undo.borrow().len(), 3);
        store.deny_lock.set(false);
        store.reject_composition.set(false);
        set_scope(editor, IS_TEXT)?;
        if case == "missing-sink" {
            scope_fixture::initialise_context(&editor.context, client)?;
        }
        editor.clear_tracking();
        pump(window)?;
        assert!(no_composition(&editor.context)?);
        assert!(live.scene()?.underlines.is_empty());
        editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
        assert!(!editor.broker.as_ref().ok_or(E_FAIL)?.1.get().active());
        // A later successful input may start only a new, one-character run.
        editor.input(WM_CHAR, WPARAM('z' as usize), LPARAM(1))?;
        pump(window)?;
        assert_eq!(live.scene()?.underlines, vec![11..12]);
        editor.clear_tracking();
        pump(window)?;
        assert!(no_composition(&editor.context)?);
        println!("Composing input / {case}: OK");
        return Ok(());
    }
    pump(window)?;
    assert_eq!(
        live.scene()?.underlines,
        vec![7..11],
        "{case}: growing underline"
    );
    let view = owned_view(&editor.context)?;
    assert_eq!(
        unsafe { view.GetOwnerClsid()? },
        sumibi_tip::CLSID_SUMIBI_TIP,
        "must be the actual DLL client"
    );
    let identity = view.cast::<IUnknown>()?;
    editor.input(WM_CHAR, WPARAM('c' as usize), LPARAM(1))?;
    pump(window)?;
    assert!(
        owned_view(&editor.context)?.cast::<IUnknown>()? == identity,
        "growth keeps the exact composition object"
    );
    assert_eq!(live.scene()?.underlines, vec![7..12]);
    if matches!(case, "shrink" | "erase") {
        editor.input(WM_CHAR, WPARAM(0x08), LPARAM(1))?;
        editor.input(WM_CHAR, WPARAM(0x08), LPARAM(1))?;
        pump(window)?;
        assert_eq!(live.scene()?.underlines, vec![7..10]);
        assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
        editor.input(WM_CHAR, WPARAM(0x08), LPARAM(1))?;
        pump(window)?;
        assert_eq!(
            live.scene()?.underlines,
            vec![7..8],
            "surrogate backspace shrinks by two units"
        );
        if case == "erase" {
            editor.input(WM_CHAR, WPARAM(0x08), LPARAM(1))?;
            pump(window)?;
            assert!(live.scene()?.underlines.is_empty());
            assert!(no_composition(&editor.context)?);
            assert_eq!(
                *store.text.borrow(),
                "prefix  suffix".encode_utf16().collect::<Vec<_>>()
            );
        }
    }
    let mut foreign = None;
    match case {
        "move" => {
            editor.input(WM_KEYDOWN, WPARAM(0x24), LPARAM(1))?;
        }
        "external-restored" => {
            let original = store.text.borrow().clone();
            let mut changed = original.clone();
            changed[0] = 'P' as u16;
            store.external_text(changed)?;
            pump(window)?;
            store.external_text(original)?;
        }
        "foreign" => {
            foreign = Some(super::super::composition_host::Composition::start(
                &editor.context,
                client,
                0,
                0,
            )?);
        }
        "empty-cycle" => {
            super::super::composition_host::empty_cycle(&editor.context, client)?;
        }
        "forced" | "forced-foreign" => unsafe {
            editor
                .context
                .cast::<ITfContextOwnerCompositionServices>()?
                .TerminateComposition(&owned_view(&editor.context)?)?;
            if case == "forced-foreign" {
                foreign = Some(super::super::composition_host::Composition::start(
                    &editor.context,
                    client,
                    7,
                    5,
                )?);
            }
        },
        "private" | "private-restored" => {
            set_scope(editor, IS_PRIVATE)?;
            if case == "private-restored" {
                pump(window)?;
                set_scope(editor, IS_TEXT)?;
            }
        }
        "denied" => store.deny_lock.set(true),
        "readonly" => store.readonly.set(true),
        "focus" => {
            editor.input(WM_KILLFOCUS, WPARAM(0), LPARAM(0))?;
        }
        "explicit" => {
            editor.input(WM_KEYDOWN, WPARAM(0x24), LPARAM(1))?;
            store.host_selection(7, 12)?;
        }
        "resize-denied" => {
            store.deny_lock.set(true);
            editor.input(WM_CHAR, WPARAM('x' as usize), LPARAM(1))?;
            assert_eq!(
                *store.text.borrow(),
                "prefix a😀bcx suffix".encode_utf16().collect::<Vec<_>>()
            );
            store.deny_lock.set(false);
            editor.input(WM_CHAR, WPARAM('z' as usize), LPARAM(1))?;
            pump(window)?;
            assert_eq!(
                live.scene()?.underlines,
                vec![13..14],
                "only new input after cleanup may be tracked"
            );
        }
        "external-new" => {
            let original = store.text.borrow().clone();
            store.external_text(original)?;
            pump(window)?;
            editor.input(WM_CHAR, WPARAM('z' as usize), LPARAM(1))?;
            pump(window)?;
            assert_eq!(live.scene()?.underlines, vec![12..13]);
        }
        _ => {}
    }
    pump(window)?;
    let original = store.text.borrow().clone();
    store.undo.borrow_mut().clear();
    if case == "close" {
        test_broker_sent_close(window, editor)?;
        assert!(no_composition(&editor.context)?);
        assert_eq!(*store.text.borrow(), original);
        assert!(store.undo.borrow().is_empty());
        println!("Composing input / {case}: OK");
        return Ok(());
    }
    let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
    if matches!(case, "gate-source" | "gate-selection" | "body-gate") {
        let composing = editor.composing.as_ref().ok_or(E_FAIL)?;
        if case == "body-gate" {
            store.host_selection(7, 12)?;
        } else {
            assert!(composing.prepare(editor, bridge)?);
        }
        if case == "gate-source" {
            let transport = bridge.to_interface::<ISpikeBrokerTransport>();
            let mut request = 0;
            let wrong: Vec<u16> = "wrong".encode_utf16().collect();
            unsafe {
                transport.Reserve(&mut request).ok()?;
                assert!(transport
                    .Start(request, 1, 1, 1, 3000, wrong.as_ptr(), wrong.len() as u32)
                    .is_err());
            }
        } else {
            if case == "gate-selection" {
                store.host_selection(8, 12)?;
            }
            let transport = bridge.to_interface::<ISpikeBrokerTransport>();
            unsafe {
                broker
                    .Attach(editor.context.as_raw(), transport.as_raw())
                    .ok()?;
                let _ = editor
                    .keys
                    .SimulatePreservedKey(&editor.context, &COMMAND_CONVERT)?;
            }
        }
        bridge.get().clear_input_check();
        assert!(
            !bridge.get().active(),
            "{case}: source cannot start a worker"
        );
        if case == "body-gate" {
            assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
        } else {
            editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
            assert!(!bridge.get().active());
        }
        assert_eq!(*store.text.borrow(), original);
        assert!(store.undo.borrow().is_empty());
    } else if matches!(
        case,
        "apply"
            | "shrink"
            | "escape"
            | "enter"
            | "explicit"
            | "external-new"
            | "resize-denied"
            | "pending-edit"
    ) {
        editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
        assert!(bridge.get().active(), "{case}");
        assert!(no_composition(&editor.context)?);
        if case == "pending-edit" {
            unsafe {
                PostMessageW(
                    Some(window.hwnd()),
                    WM_CHAR,
                    WPARAM('x' as usize),
                    LPARAM(1),
                )?;
            }
        } else if matches!(case, "escape" | "enter") {
            unsafe {
                PostMessageW(
                    Some(window.hwnd()),
                    WM_KEYDOWN,
                    WPARAM(if case == "escape" { 0x1b } else { 0x0d }),
                    LPARAM(1),
                )?;
            }
        }
        tracking_host::wait_result(
            window,
            broker,
            bridge,
            !matches!(case, "escape" | "enter" | "pending-edit"),
        )?;
        if matches!(
            case,
            "apply" | "shrink" | "explicit" | "external-new" | "resize-denied"
        ) {
            let expected = match case {
                "external-new" => "prefix a😀bc日本語 suffix",
                "resize-denied" => "prefix a😀bcx日本語 suffix",
                _ => "prefix 日本語 suffix",
            };
            assert_eq!(
                *store.text.borrow(),
                expected.encode_utf16().collect::<Vec<_>>()
            );
            assert_eq!(store.undo.borrow().len(), 1);
            if case != "explicit" {
                editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
                assert!(
                    !bridge.get().active(),
                    "no implicit re-conversion of an auto-selected range"
                );
            }
            editor.input(WM_CHAR, WPARAM(0x1a), LPARAM(1))?;
            assert_eq!(*store.text.borrow(), original);
        } else if case == "pending-edit" {
            assert_eq!(
                *store.text.borrow(),
                "prefix x suffix".encode_utf16().collect::<Vec<_>>()
            );
            pump(window)?;
            assert_eq!(live.scene()?.underlines, vec![7..8]);
        } else {
            assert_eq!(*store.text.borrow(), original);
            assert!(store.undo.borrow().is_empty());
        }
        assert_eq!(editor.submits.get(), usize::from(case == "enter"));
    } else if case != "grow" {
        editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
        assert!(!bridge.get().active(), "{case}: stale range rejected");
        assert_eq!(*store.text.borrow(), original);
        assert!(store.undo.borrow().is_empty());
    }
    store.deny_lock.set(false);
    store.readonly.set(false);
    *store.scopes.borrow_mut() = ScopeSpec::List(vec![IS_TEXT]);
    editor.clear_tracking();
    if let Some(foreign) = &mut foreign {
        assert!(!no_composition(&editor.context)?);
        foreign.end()?;
    }
    pump(window)?;
    assert!(no_composition(&editor.context)?, "{case}: cleanup");
    assert!(
        live.scene()?.underlines.is_empty(),
        "{case}: underline cleanup"
    );
    println!("Composing input / {case}: OK");
    Ok(())
}
fn set_scope(editor: &Editor, scope: InputScope) -> Result<()> {
    *editor.store.get().scopes.borrow_mut() = ScopeSpec::List(vec![scope]);
    let sink = editor.store.get().sink.borrow().clone().ok_or(E_FAIL)?;
    let length = editor.store.get().text.borrow().len() as i32;
    unsafe { sink.OnAttrsChange(0, length, &[GUID_PROP_INPUTSCOPE]) }
}
fn owned_view(context: &ITfContext) -> Result<ITfCompositionView> {
    let service: ITfContextComposition = context.cast()?;
    let enumerator = unsafe { service.EnumCompositions()? };
    let mut slot = [None];
    let mut count = 0;
    unsafe {
        enumerator.Next(&mut slot, &mut count)?;
    }
    if count != 1 {
        return Err(E_FAIL.into());
    }
    slot[0].take().ok_or_else(|| E_FAIL.into())
}
pub(super) fn no_composition(context: &ITfContext) -> Result<bool> {
    let service: ITfContextComposition = context.cast()?;
    let enumerator = unsafe { service.EnumCompositions()? };
    let mut slot = [None];
    let mut count = 0;
    unsafe {
        enumerator.Next(&mut slot, &mut count)?;
    }
    Ok(count == 0 && slot[0].is_none())
}
