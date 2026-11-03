//! Autonomous logon-scoped Host discovery, TipHostSession lifecycle,
//! preserved key conversion, async STA drain window text replacement, and Undo.
use super::*;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};
use sumibi_host::host_control::HostControl;
use sumibi_host::host_service::HostService;
use sumibi_host::manual::{Credential, Destination, Scheme};
use sumibi_host::policy::PolicyAuthority;
use sumibi_tip::spike::COMMAND_CONVERT;
use windows::core::{ComObject, Interface};
use windows::Win32::Foundation::E_FAIL;

type Failure = Box<dyn std::error::Error>;

pub(crate) fn exercise_session_keys(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Failure> {
    let messages: ITfMessagePump = manager.cast()?;

    // 0. Setup local diagnostic HTTP server for chat completions mock
    let response_body = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": r#"{"candidates":["日本語"]}"#
            }
        }]
    })
    .to_string();
    let response_bytes = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
        response_body.len()
    )
    .into_bytes();

    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind listener: {e:?}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local addr: {e:?}"))?
        .port();

    let _server_thread = std::thread::spawn(move || {
        let (mut stream, _) = match listener.accept() {
            Ok(res) => res,
            Err(_) => return,
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(4)));
        let mut request = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    request.extend_from_slice(&chunk[..n]);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
            }
        }
        let _ = stream.write_all(&response_bytes);
    });

    // 1. Setup diagnostic HostService (unconsented initially)
    let destination = Destination::for_chat_completions(Scheme::Https, "127.0.0.1", port, "/")
        .ok_or("invalid fixture destination")?;
    let authority = Arc::new(PolicyAuthority::new(destination));
    authority
        .set_model("fixture-model")
        .map_err(|_| "set model")?;
    authority
        .replace_credential(Credential::new("session-key-test".into()))
        .map_err(|_| "credential replace")?;

    let thread_authority = authority.clone();
    let control = Arc::new(
        HostControl::start_diagnostic(move || Ok((thread_authority, false)))
            .map_err(|e| format!("start HostControl: {e:?}"))?,
    );
    let mut service = HostService::start_with_control(control)
        .map_err(|e| format!("start HostService: {e:?}"))?;

    // 2. Setup window, TextStore, document and context
    let window = crate::window_host::Window::new(manager)?;
    let store = ComObject::new(TextStore::new("prefix nihongo suffix"));
    let mut selected = store.get().selection.get();
    selected.acpStart = 7;
    selected.acpEnd = 14;
    store.get().selection.set(selected);
    store.get().window.set(window.hwnd());

    let document = Document(unsafe { manager.CreateDocumentMgr()? });
    let mut context = None;
    let mut cookie = 0;
    unsafe {
        document.0.CreateContext(
            client,
            0,
            &store.to_interface::<IUnknown>(),
            &mut context,
            &mut cookie,
        )?;
    }
    let context = context.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
    unsafe {
        document.0.Push(&context)?;
    }
    scope_fixture::initialise_context(&context, client)?;
    let mut association = crate::window_host::Association::new(manager, &window, &document.0)?;
    unsafe {
        manager.SetFocus(&document.0)?;
    }
    pump_tsf_messages(&messages)?;

    let provider = unsafe { manager.GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)? };
    let sink: ITfKeyEventSink = unsafe {
        provider
            .GetFunction(
                &sumibi_tip::spike::FUNCTION_TEST_KEY_SINK,
                &ITfKeyEventSink::IID,
            )?
            .cast()?
    };

    // =========================================================================
    // Scenario 1: Unconsented Fail-Closed
    // =========================================================================
    // In unconsented state, TipHostSession worker fails discovery handshake
    // and is not ready. Pressing the preserved key must be immediately rejected
    // without reading or altering the document text body.
    let before_body_reads = store.get().body_reads.get();
    let admitted = unsafe { sink.OnPreservedKey(&context, &COMMAND_CONVERT)? };
    assert!(
        !admitted.as_bool(),
        "unconsented hotkey must be rejected (admitted={admitted:?})"
    );
    assert_eq!(
        store.get().body_reads.get(),
        before_body_reads,
        "unconsented hotkey must never read document body text"
    );
    assert_eq!(store.get().body_reads.get(), 0);
    assert!(store.get().undo.borrow().is_empty());
    assert_eq!(
        *store.get().text.borrow(),
        "prefix nihongo suffix".encode_utf16().collect::<Vec<_>>()
    );
    println!("Scenario 1: Unconsented hotkey fail-closed: OK");

    // =========================================================================
    // Scenario 2: Autonomous discovery, key conversion and async text replacement
    // =========================================================================
    // Grant consent to the host and wait until the status reflects consent.
    let consent_deadline = Instant::now() + Duration::from_secs(5);
    let mut consent_granted = false;
    let mut consent_confirmed = false;
    while Instant::now() < consent_deadline {
        if let Ok(Some(status)) = service.try_status() {
            if let Some(offer) = status.offer {
                if !consent_granted {
                    service
                        .control()
                        .try_grant(offer)
                        .map_err(|e| format!("try_grant: {e:?}"))?;
                    consent_granted = true;
                } else if offer.allowed() {
                    consent_confirmed = true;
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        consent_confirmed,
        "timed out waiting for consent to be confirmed"
    );

    // Wait for the TipHostSession worker to autonomously discover, connect,
    // and update policy to Ready. We poll OnPreservedKey until it's admitted.
    let ready_deadline = Instant::now() + Duration::from_secs(6);
    let mut key_admitted = false;
    while Instant::now() < ready_deadline {
        unsafe {
            manager.SetFocus(&document.0)?;
        }
        pump_tsf_messages(&messages)?;
        let res = unsafe { sink.OnPreservedKey(&context, &COMMAND_CONVERT)? };
        if res.as_bool() {
            key_admitted = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        key_admitted,
        "consented hotkey must be admitted by TipHostSession"
    );

    // Conversion is dispatched to the background worker. The worker communicates
    // with the Host over private pipe, receives "日本語", and posts WM_SUMIBI_DRAIN
    // to DrainWindow. Pumping messages on the STA thread executes sink.drain()
    // and updates the text store.
    let drain_deadline = Instant::now() + Duration::from_secs(6);
    let expected_replaced: Vec<u16> = "prefix 日本語 suffix".encode_utf16().collect();
    let mut replaced = false;
    while Instant::now() < drain_deadline {
        pump_tsf_messages(&messages)?;
        if *store.get().text.borrow() == expected_replaced {
            replaced = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        replaced,
        "text store must be asynchronously replaced with 'prefix 日本語 suffix' via DrainWindow"
    );
    assert_eq!(store.get().undo.borrow().len(), 1);
    println!("Scenario 2: Autonomous discovery, key conversion and async text replacement: OK");

    // =========================================================================
    // Scenario 3: 1-step Undo restores original text and selection
    // =========================================================================
    store.get().undo_once()?;
    assert_eq!(
        *store.get().text.borrow(),
        "prefix nihongo suffix".encode_utf16().collect::<Vec<_>>()
    );
    let current_selection = store.get().selection.get();
    assert_eq!(current_selection.acpStart, 7);
    assert_eq!(current_selection.acpEnd, 14);
    assert!(store.get().undo.borrow().is_empty());
    println!("Scenario 3: TextStore 1-step Undo: OK");

    // =========================================================================
    // Scenario 4: Consent Revocation Fail-Closed
    // =========================================================================
    // Revoke consent from the host.
    service
        .control()
        .try_revoke()
        .map_err(|e| format!("try_revoke: {e:?}"))?;

    let revoke_status_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < revoke_status_deadline {
        if let Ok(Some(status)) = service.try_status() {
            if status.offer.as_ref().is_some_and(|o| !o.allowed()) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // The TipHostSession worker will observe PolicyDenied / disconnection,
    // clear ready state, and return to reconnection backoff.
    // Preserved key must once again be immediately rejected without reading.
    let revoke_deadline = Instant::now() + Duration::from_secs(6);
    let mut key_rejected = false;
    let body_reads_before_revocation = store.get().body_reads.get();
    while Instant::now() < revoke_deadline {
        pump_tsf_messages(&messages)?;
        let res = unsafe { sink.OnPreservedKey(&context, &COMMAND_CONVERT)? };
        if !res.as_bool() {
            key_rejected = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(key_rejected, "hotkey must be rejected after revocation");
    assert_eq!(
        store.get().body_reads.get(),
        body_reads_before_revocation,
        "revoked hotkey must never read document body text"
    );
    assert_eq!(
        *store.get().text.borrow(),
        "prefix nihongo suffix".encode_utf16().collect::<Vec<_>>()
    );
    println!("Scenario 4: Revocation fail-closed: OK");

    // =========================================================================
    // Scenario 5: Teardown and Cleanup
    // =========================================================================
    association.close()?;
    unsafe {
        manager.SetFocus(None)?;
    }
    pump_tsf_messages(&messages)?;
    service.stop().map_err(|e| format!("service stop: {e:?}"))?;
    println!("Scenario 5: Teardown and cleanup: OK");

    Ok(())
}
