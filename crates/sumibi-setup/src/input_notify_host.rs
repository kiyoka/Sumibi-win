//! Native metadata wake tests. No further input is needed to end a stale run.
use super::*;
use sumibi_tip::spike::ISpikeInputSignal_Impl;
use windows::Win32::Foundation::{E_FAIL, WAIT_OBJECT_0};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

pub(crate) fn exercise_notifications(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for case in [
        "own-notices",
        "text",
        "text-restored",
        "selection",
        "selection-restored",
        "private",
        "private-restored",
        "compartment-restored",
        "readonly",
        "readonly-restored",
        "lock-denied",
        "foreign",
        "empty-cycle",
        "forced",
        "focus",
        "burst",
        "reattach",
        "late-close",
        "failed-signal",
        "pending-status-restored",
    ] {
        run(manager, client, RunMode::NotificationTest(case))?;
    }
    Ok(())
}
pub(in crate::edit_host::input_host) fn test_notifications(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
    client: u32,
    case: &str,
) -> Result<()> {
    let store = editor.store.get();
    let composing = editor.composing.as_ref().ok_or(E_FAIL)?;
    let (broker, bridge) = editor.broker.as_ref().ok_or(E_FAIL)?;
    store.external_text("prefix  suffix".encode_utf16().collect())?;
    store.host_selection(7, 7)?;
    pump(window)?;
    for unit in "nihongo".encode_utf16() {
        editor.input(WM_CHAR, WPARAM(unit.into()), LPARAM(1))?;
    }
    pump(window)?;
    assert!(drain(window, editor)? <= 2);
    assert_eq!(
        live.scene()?.underlines,
        vec![7..14],
        "own notices must preserve a settled run"
    );
    let identity = owned_view(&editor.context)?.cast::<IUnknown>()?;
    let original = store.text.borrow().clone();
    store.undo.borrow_mut().clear();
    let body_before = body_reads()?;
    if case == "pending-status-restored" {
        editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
        assert!(bridge.get().active());
        let captured = body_reads()?;
        store.readonly.set(true);
        notify_status(editor, TS_SD_READONLY)?;
        store.readonly.set(false);
        notify_status(editor, 0)?;
        wait_signal(window, composing.wait_handle().ok_or(E_FAIL)?)?;
        drain(window, editor)?;
        tracking_host::wait_result(window, broker, bridge, false)?;
        assert_eq!(*store.text.borrow(), original);
        assert!(store.undo.borrow().is_empty());
        assert_eq!(
            body_reads()?,
            captured,
            "status history rejects before body reread/apply"
        );
        println!("Input owner wake / {case}: OK");
        return Ok(());
    }
    if case == "own-notices" || case == "failed-signal" {
        if case == "failed-signal" {
            let failed: ISpikeInputSignal = FailingSignal.into();
            unsafe {
                composing.monitor.SetSignal(failed.as_raw()).ok()?;
            }
        }
        editor.input(WM_CHAR, WPARAM('x' as usize), LPARAM(1))?;
        pump(window)?;
        if case == "failed-signal" {
            unsafe {
                composing
                    .monitor
                    .SetSignal(
                        composing
                            .signal
                            .to_interface::<ISpikeInputSignal>()
                            .as_raw(),
                    )
                    .ok()?;
            }
        }
        drain(window, editor)?;
        assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
        assert_eq!(live.scene()?.underlines, vec![7..15]);
        assert_eq!(
            *store.text.borrow(),
            "prefix nihongox suffix".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(
            store.undo.borrow().len(),
            1,
            "a failing signal never fails/replays host input"
        );
        assert_eq!(body_reads()?, body_before);
        editor.clear_tracking();
        drain(window, editor)?;
        assert!(no_composition(&editor.context)?);
        println!("Input owner wake / {case}: OK");
        return Ok(());
    }
    if case == "late-close" {
        let late = composing.signal.to_interface::<ISpikeInputSignal>();
        composing.close(editor)?;
        unsafe {
            late.Changed().ok()?;
        }
        assert!(composing.wait_handle().is_none());
        assert!(!composing.signal.get().drain()?);
        assert!(no_composition(&editor.context)?);
        assert_eq!(*store.text.borrow(), original);
        assert!(store.undo.borrow().is_empty());
        assert_eq!(body_reads()?, body_before);
        println!("Input owner wake / {case}: OK");
        return Ok(());
    }
    let mut foreign = None;
    let mut previous_document = None;
    match case {
        "text" | "text-restored" => {
            let mut changed = original.clone();
            changed[7] = 'N' as u16;
            store.external_text(changed)?;
            if case == "text-restored" {
                store.external_text(original.clone())?;
            }
        }
        "selection" | "selection-restored" => {
            store.host_selection(0, 0)?;
            if case == "selection-restored" {
                store.host_selection(14, 14)?;
            }
        }
        "private" | "private-restored" => {
            set_scope(editor, IS_PRIVATE)?;
            if case == "private-restored" {
                set_scope(editor, IS_TEXT)?;
            }
        }
        "compartment-restored" => {
            let compartments: ITfCompartmentMgr = editor.context.cast()?;
            unsafe {
                let compartment =
                    compartments.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)?;
                compartment.SetValue(
                    client,
                    &windows::Win32::System::Variant::VARIANT::from(1i32),
                )?;
                compartment.SetValue(
                    client,
                    &windows::Win32::System::Variant::VARIANT::from(0i32),
                )?;
            }
        }
        "readonly" | "readonly-restored" => {
            store.readonly.set(true);
            notify_status(editor, TS_SD_READONLY)?;
            if case == "readonly-restored" {
                store.readonly.set(false);
                notify_status(editor, 0)?;
            }
        }
        "lock-denied" => {
            store.dynamic_status.set(TS_SD_LOADING);
            notify_status(editor, TS_SD_LOADING)?;
            // Deliver the real status notification while reads are possible;
            // then deny cleanup before the owner processes its queued wake.
            pump(window)?;
            store.deny_lock.set(true);
        }
        "foreign" => {
            foreign = Some(super::super::super::composition_host::Composition::start(
                &editor.context,
                client,
                0,
                0,
            )?);
        }
        "empty-cycle" => {
            super::super::super::composition_host::empty_cycle(&editor.context, client)?;
        }
        "forced" => unsafe {
            editor
                .context
                .cast::<ITfContextOwnerCompositionServices>()?
                .TerminateComposition(&owned_view(&editor.context)?)?;
        },
        "focus" => {
            let manager: ITfThreadMgr = editor.keys.cast()?;
            let document = unsafe { editor.context.GetDocumentMgr()? };
            unsafe {
                manager.SetFocus(None)?;
            }
            previous_document = Some((manager, document));
        }
        "burst" => {
            for _ in 0..1000 {
                composing.signal.get().notify()?;
            }
            assert!(drain(window, editor)? <= 2);
            assert!(owned_view(&editor.context)?.cast::<IUnknown>()? == identity);
            for _ in 0..32 {
                set_scope(editor, IS_PRIVATE)?;
                set_scope(editor, IS_TEXT)?;
            }
        }
        "reattach" => {
            unsafe {
                composing.monitor.SetSignal(std::ptr::null_mut()).ok()?;
            }
            store.input_signal.borrow_mut().take();
            composing.signal.get().drain()?;
            store.host_selection(0, 0)?;
            pump(window)?;
            assert!(
                !composing.signal.get().drain()?,
                "detached provider must not signal"
            );
            composing.connect(editor);
            unsafe {
                composing
                    .monitor
                    .SetSignal(
                        composing
                            .signal
                            .to_interface::<ISpikeInputSignal>()
                            .as_raw(),
                    )
                    .ok()?;
            }
        }
        _ => unreachable!(),
    }
    pump(window)?;
    let handle = composing.wait_handle().ok_or(E_FAIL)?;
    wait_signal(window, handle)?;
    // Merely receiving a notification must not synchronously edit the document.
    if !matches!(case, "forced" | "focus") {
        assert!(
            !no_composition(&editor.context)?,
            "cleanup runs only after returning to owner loop"
        );
    }
    assert!(drain(window, editor)? <= 3, "coalesced wake must not spin");
    if matches!(case, "readonly" | "lock-denied") {
        assert!(
            !no_composition(&editor.context)?,
            "denied cleanup retains one owned object"
        );
        assert!(
            !composing.signal.get().drain()?,
            "denied cleanup must not resignal itself"
        );
        let lock_requests = store.lock_requests.get();
        for _ in 0..1000 {
            composing.signal.get().notify()?;
        }
        // No message pump here: status/layout work from Windows may issue
        // unrelated read locks. These calls observe the same notification era.
        assert!(composing.poll(editor)?);
        assert!(!composing.poll(editor)?);
        assert_eq!(
            store.lock_requests.get(),
            lock_requests,
            "same-generation wakes cannot retry a failed lock"
        );
        store.readonly.set(false);
        store.deny_lock.set(false);
        store.dynamic_status.set(0);
        notify_status(editor, 0)?;
        pump(window)?;
        assert!(
            drain(window, editor)? > 0,
            "status recovery must wake cleanup without a key"
        );
    }
    if let Some((manager, document)) = previous_document {
        unsafe {
            manager.SetFocus(&document)?;
        }
    }
    if let Some(foreign) = &mut foreign {
        assert!(!no_composition(&editor.context)?);
        assert_ne!(
            unsafe { owned_view(&editor.context)?.GetOwnerClsid()? },
            sumibi_tip::CLSID_SUMIBI_TIP
        );
        foreign.end()?;
    }
    set_scope(editor, IS_TEXT)?;
    drain(window, editor)?;
    assert!(
        no_composition(&editor.context)?,
        "{case}: no more input required for cleanup"
    );
    assert!(live.scene()?.underlines.is_empty(), "{case}");
    let expected = if case == "text" {
        "prefix Nihongo suffix".encode_utf16().collect::<Vec<_>>()
    } else {
        original
    };
    assert_eq!(*store.text.borrow(), expected);
    assert!(
        store.undo.borrow().is_empty(),
        "metadata cleanup cannot add body undo"
    );
    assert_eq!(
        body_reads()?,
        body_before,
        "actual DLL cleanup cannot read body"
    );
    assert!(!bridge.get().active());
    editor.input(WM_KEYDOWN, WPARAM(0x76), LPARAM(1))?;
    assert!(
        !bridge.get().active(),
        "{case}: waking must retire old conversion rights"
    );
    let mut pending = 1;
    unsafe {
        broker.Pending(&mut pending).ok()?;
    }
    assert_eq!(pending, 0);
    println!("Input owner wake / {case}: OK");
    Ok(())
}
fn drain(window: &crate::window_host::Window, editor: &Editor) -> Result<usize> {
    let composing = editor.composing.as_ref().ok_or(E_FAIL)?;
    for count in 0..4 {
        pump(window)?;
        if !composing.poll(editor)? {
            return Ok(count);
        }
    }
    Err(E_FAIL.into())
}
fn wait_signal(window: &crate::window_host::Window, handle: HANDLE) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        pump(window)?;
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(E_FAIL.into());
        }
        let ready = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[handle]),
                remaining.as_millis() as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if ready == WAIT_OBJECT_0 {
            return Ok(());
        }
        if ready == windows::Win32::Foundation::WAIT_FAILED
            || ready == windows::Win32::Foundation::WAIT_TIMEOUT
        {
            return Err(E_FAIL.into());
        }
    }
}
fn notify_status(editor: &Editor, flags: u32) -> Result<()> {
    let sink = editor.store.get().sink.borrow().clone().ok_or(E_FAIL)?;
    unsafe { sink.OnStatusChange(flags) }
}
pub(super) fn body_reads() -> Result<u64> {
    // Borrow the already-loaded build artifact, retained by probe throughout
    // this synchronous test. Do not read the statically linked EXE counter.
    unsafe {
        let module = GetModuleHandleW(windows::core::w!("sumibi_tip.dll"))?;
        let address =
            GetProcAddress(module, windows::core::s!("SumibiSpikeBodyReadCount")).ok_or(E_FAIL)?;
        let count: extern "system" fn() -> u64 = std::mem::transmute(address);
        Ok(count())
    }
}
#[implement(ISpikeInputSignal)]
struct FailingSignal;
impl ISpikeInputSignal_Impl for FailingSignal_Impl {
    unsafe fn Changed(&self) -> HRESULT {
        E_FAIL
    }
}
