//! Deterministically delayed responses; worker threads never receive COM data.
use super::*;
use std::sync::mpsc::{sync_channel, TryRecvError};
use std::time::{Duration, Instant};
use sumibi_tip::spike::{
    ISpikeControl, ISpikeDelayed, FUNCTION_TEST_CONTROL, FUNCTION_TEST_DELAYED,
};
use windows::Win32::Foundation::{LPARAM, WPARAM};

pub(crate) fn exercise_delayed(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    // SAFETY: every COM object and synthetic document remains on this STA.
    unsafe {
        for (index, case) in [
            "apply",
            "escape",
            "enter",
            "text",
            "selection",
            "focus",
            "revoke",
            "readonly",
            "write-denied",
            "timeout",
            "new-request",
        ]
        .into_iter()
        .enumerate()
        {
            let request = index as u64 * 3 + 1;
            let window = crate::window_host::Window::new()?;
            let store = ComObject::new(TextStore::new("prefix nihongo suffix"));
            let mut selected = store.get().selection.get();
            selected.acpStart = 7;
            selected.acpEnd = 14;
            store.get().selection.set(selected);
            store.get().window.set(window.hwnd());
            let document = Document(manager.CreateDocumentMgr()?);
            let mut context = None;
            let mut cookie = 0;
            document.0.CreateContext(
                client,
                0,
                &store.to_interface::<IUnknown>(),
                &mut context,
                &mut cookie,
            )?;
            let context = context.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
            document.0.Push(&context)?;
            scope_fixture::initialise_context(&context, client)?;
            let mut association =
                crate::window_host::Association::new(manager, &window, &document.0)?;
            window.focus()?;
            manager.SetFocus(&document.0)?;
            let provider = manager.GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)?;
            let control: ISpikeControl = provider
                .GetFunction(&FUNCTION_TEST_CONTROL, &ISpikeControl::IID)?
                .cast()?;
            let delayed: ISpikeDelayed = provider
                .GetFunction(&FUNCTION_TEST_DELAYED, &ISpikeDelayed::IID)?
                .cast()?;
            control.GrantContext(context.as_raw()).ok()?;
            assert!(delayed.BeginRequest(request, 0).is_err());
            assert!(delayed.BeginRequest(request, 2001).is_err());
            delayed
                .BeginRequest(request, if case == "timeout" { 10 } else { 2000 })
                .ok()?;
            assert!(delayed.BeginRequest(request + 1, 2000).is_err()); // Cannot overwrite pending work.
            let mut other = None;
            let mut other_store = None;

            // The worker waits until the owner has changed the target or handled
            // a key. Bounded channels prevent queued responses from accumulating.
            std::thread::scope(
                |scope| -> std::result::Result<(), Box<dyn std::error::Error>> {
                    let (release, wait) = sync_channel(1);
                    let (send, receive) = sync_channel(1);
                    let worker = scope.spawn(move || {
                        if wait.recv_timeout(Duration::from_secs(2)).is_ok() {
                            if case == "timeout" {
                                std::thread::sleep(Duration::from_millis(25));
                            }
                            let _ = send.send(
                                sumibi_protocol::encode(&request.to_string())
                                    .expect("fixed fixture"),
                            );
                        }
                    });
                    pump_tsf_messages()?;
                    control.VerifyPreview().ok()?;
                    match case {
                        "escape" | "enter" => {
                            let key = WPARAM(if case == "escape" { 0x1b } else { 0x0d });
                            let keys: ITfKeystrokeMgr = manager.cast()?;
                            assert!(keys.TestKeyDown(key, LPARAM(1))?.as_bool());
                            assert_eq!(keys.KeyDown(key, LPARAM(1))?.as_bool(), case == "escape");
                        }
                        "text" => store
                            .get()
                            .external_text("prefix NIHONGO suffix".encode_utf16().collect())?,
                        "selection" => {
                            let mut selection = store.get().selection.get();
                            selection.acpEnd = 10;
                            store.get().selection.set(selection);
                            let sink = store
                                .get()
                                .sink
                                .borrow()
                                .clone()
                                .ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
                            sink.OnSelectionChange()?;
                        }
                        "focus" => {
                            let next = Document(manager.CreateDocumentMgr()?);
                            let next_store =
                                ComObject::new(TextStore::new("prefix nihongo suffix"));
                            let mut next_context = None;
                            let mut next_cookie = 0;
                            next.0.CreateContext(
                                client,
                                0,
                                &next_store.to_interface::<IUnknown>(),
                                &mut next_context,
                                &mut next_cookie,
                            )?;
                            next.0.Push(
                                &next_context
                                    .ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?,
                            )?;
                            manager.SetFocus(&next.0)?;
                            other = Some(next);
                            other_store = Some(next_store);
                        }
                        "revoke" => {
                            control.RevokeContext().ok()?;
                        }
                        "readonly" => store.get().readonly.set(true),
                        "write-denied" => store.get().deny_lock.set(true),
                        "new-request" => {
                            control.CancelPreview().ok()?;
                            delayed.BeginRequest(request + 1, 2000).ok()?;
                        }
                        _ => {}
                    }
                    let before = store.get().text.borrow().clone();
                    release.send(())?;
                    let deadline = Instant::now() + Duration::from_secs(2);
                    let bytes = loop {
                        pump_tsf_messages()?;
                        match receive.try_recv() {
                            Ok(bytes) => break bytes,
                            Err(TryRecvError::Empty) if Instant::now() < deadline => {
                                std::thread::sleep(Duration::from_millis(1))
                            }
                            _ => return Err("synthetic response did not arrive".into()),
                        }
                    };
                    worker
                        .join()
                        .map_err(|_| "synthetic response worker failed")?;
                    let mut decoder = sumibi_protocol::Decoder::default();
                    let response = decoder
                        .decode(&bytes)
                        .map_err(|_| "invalid synthetic frame")?;
                    assert_eq!(response.consumed, bytes.len());
                    let request: u64 =
                        response.body.ok_or("incomplete synthetic frame")?.parse()?;
                    let result = delayed.CompleteRequest(request);
                    if case == "apply" {
                        result.ok()?;
                        assert_eq!(
                            *store.get().text.borrow(),
                            "prefix 日本語 suffix".encode_utf16().collect::<Vec<_>>()
                        );
                        assert!(delayed.CompleteRequest(request).is_err());
                        assert_eq!(store.get().undo.borrow().len(), 1);
                        store.get().undo_once()?;
                        assert_eq!(*store.get().text.borrow(), before);
                    } else {
                        assert!(result.is_err(), "{case}");
                        assert_eq!(*store.get().text.borrow(), before, "{case}");
                        assert!(store.get().undo.borrow().is_empty(), "{case}");
                        if case == "new-request" {
                            control.VerifyPreview().ok()?;
                            delayed.CompleteRequest(request + 1).ok()?;
                            assert_eq!(
                                *store.get().text.borrow(),
                                "prefix 日本語 suffix".encode_utf16().collect::<Vec<_>>()
                            );
                        }
                    }
                    assert!(delayed.BeginRequest(request, 2000).is_err()); // Never reuse a retired ID.
                    Ok(())
                },
            )?;
            if let Some(next) = &other_store {
                assert_eq!(
                    *next.get().text.borrow(),
                    "prefix nihongo suffix".encode_utf16().collect::<Vec<_>>()
                );
            }
            store.get().readonly.set(false);
            store.get().deny_lock.set(false);
            control.RevokeContext().ok()?;
            pump_tsf_messages()?;
            association.close()?;
            manager.SetFocus(None)?;
            drop(other);
            println!("Delayed DLL response / {case}: OK");
        }
    }
    Ok(())
}
