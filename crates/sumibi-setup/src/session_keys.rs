//! Autonomous logon-scoped Host discovery, TipHostSession lifecycle,
//! preserved key conversion, async STA drain window text replacement, and Undo.
use super::*;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use sumibi_host::host_control::HostControl;
use sumibi_host::host_service::HostService;
use sumibi_host::manual::{Credential, Destination, PolicySource, Scheme};
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

    // 0. Setup local diagnostic HTTP server with switchable response modes:
    //    0: 200 OK ("日本語")
    //    1: 500 Internal Server Error
    let response_body_200 = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": r#"{"candidates":["日本語"]}"#
            }
        }]
    })
    .to_string();
    let response_bytes_200 = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body_200}",
        response_body_200.len()
    )
    .into_bytes();

    let response_body_500 = r#"{"error":{"message":"internal simulated failure"}}"#;
    let response_bytes_500 = format!(
        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body_500}",
        response_body_500.len()
    )
    .into_bytes();

    let server_mode = Arc::new(AtomicU8::new(0)); // 0: 200 OK, 1: 500 Error
    let thread_mode = server_mode.clone();
    let server_running = Arc::new(AtomicBool::new(true));
    let thread_running = server_running.clone();

    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind listener: {e:?}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("set nonblocking: {e:?}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local addr: {e:?}"))?
        .port();

    let _server_thread = std::thread::spawn(move || {
        while thread_running.load(Ordering::Relaxed) {
            let (mut stream, _) = match listener.accept() {
                Ok(res) => res,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(_) => break,
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
            let mode = thread_mode.load(Ordering::Relaxed);
            if mode == 0 {
                let _ = stream.write_all(&response_bytes_200);
            } else if mode == 1 {
                let _ = stream.write_all(&response_bytes_500);
            }
        }
    });

    // 1. Setup diagnostic HostService (unconsented initially)
    let destination = Destination::for_chat_completions(Scheme::Https, "127.0.0.1", port, "/")
        .ok_or("invalid fixture destination")?;
    let authority = Arc::new(PolicyAuthority::new(destination.clone()));
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
    window
        .focus()
        .map_err(|error| format!("window focus: {error}"))?;
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
    // Scenario 4: Communication Failure Preserves Original Text
    // =========================================================================
    // Configure mock server to return HTTP 500 error.
    server_mode.store(1, Ordering::Release);

    // Reset selection to "nihongo"
    let mut selected_retry = store.get().selection.get();
    selected_retry.acpStart = 7;
    selected_retry.acpEnd = 14;
    store.get().selection.set(selected_retry);

    let res_fail = unsafe { sink.OnPreservedKey(&context, &COMMAND_CONVERT)? };
    assert!(
        res_fail.as_bool(),
        "hotkey while ready must be admitted for dispatch even if upstream server subsequently fails"
    );

    // Pump messages until worker receives 500, delivers Failed event, and cancels ticket
    let fail_deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < fail_deadline {
        pump_tsf_messages(&messages)?;
        std::thread::sleep(Duration::from_millis(20));
    }

    // Original text must remain completely unchanged, and no corrupt undo state created
    assert_eq!(
        *store.get().text.borrow(),
        "prefix nihongo suffix".encode_utf16().collect::<Vec<_>>(),
        "text must remain original upon conversion failure"
    );
    assert!(
        store.get().undo.borrow().is_empty(),
        "undo history must remain empty on conversion failure"
    );
    println!("Scenario 4: Communication failure preserves original text: OK");

    // =========================================================================
    // Scenario 5: Immediate Retry Without Error Dismissal Succeeds
    // =========================================================================
    // No error modal dismissal or reset operation needed.
    // Switch server back to 200 OK.
    server_mode.store(0, Ordering::Release);

    // Immediately trigger conversion on the same selection
    let res_retry = unsafe { sink.OnPreservedKey(&context, &COMMAND_CONVERT)? };
    assert!(
        res_retry.as_bool(),
        "immediate retry must be admitted without manual error dismissal"
    );

    let retry_drain_deadline = Instant::now() + Duration::from_secs(6);
    let mut retry_replaced = false;
    while Instant::now() < retry_drain_deadline {
        pump_tsf_messages(&messages)?;
        if *store.get().text.borrow() == expected_replaced {
            retry_replaced = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        retry_replaced,
        "immediate retry must successfully replace text with 'prefix 日本語 suffix'"
    );
    assert_eq!(store.get().undo.borrow().len(), 1);

    // Revert text back via Undo for remaining scenarios
    store.get().undo_once()?;
    assert_eq!(
        *store.get().text.borrow(),
        "prefix nihongo suffix".encode_utf16().collect::<Vec<_>>()
    );
    println!("Scenario 5: Immediate retry without error dismissal succeeds: OK");

    // =========================================================================
    // Scenario 6: Consent Revocation Fail-Closed
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
    println!("Scenario 6: Revocation fail-closed: OK");

    // =========================================================================
    // Scenario 7: Missing Credential Fail-Closed
    // =========================================================================
    // Clear credential in the authority. With no credential, consent cannot be granted.
    authority
        .replace_credential(None)
        .map_err(|_| "credential remove")?;
    assert!(authority.snapshot().unwrap().credential.is_none());
    assert!(!authority.snapshot().unwrap().allowed);

    // TipHostSession must remain un-ready and hotkey must be denied without text access
    let body_reads_missing_cred = store.get().body_reads.get();
    let res_no_cred = unsafe { sink.OnPreservedKey(&context, &COMMAND_CONVERT)? };
    assert!(
        !res_no_cred.as_bool(),
        "hotkey with missing credential must be denied"
    );
    assert_eq!(
        store.get().body_reads.get(),
        body_reads_missing_cred,
        "missing credential must never read document text"
    );
    println!("Scenario 7: Missing credential fail-closed: OK");

    // =========================================================================
    // Scenario 8: Candidate list keyboard navigation, selection and 1-step Undo
    // =========================================================================
    // ADR 0099, ADR 0100, ADR 0101: Test candidate session keyboard navigation,
    // digit selection, annotation stripping from document text, and 1-step undo.
    use sumibi_core::candidate::{CandidateItem, CandidateSource};
    use sumibi_tip::candidate_session::CandidateSession;

    // Simulate candidates returned from host (LLM + Mozc + Sudachi + Nakaguro)
    let candidates = vec![
        CandidateItem::new("更新", CandidateSource::CurrentResult),
        CandidateItem::new("交信", CandidateSource::Llm),
        CandidateItem::new("行進", CandidateSource::SudachiHomophone),
        CandidateItem::with_annotation("⇒", CandidateSource::MozcSymbol, "記号"),
    ];

    let mut candidate_session =
        CandidateSession::new("koushin".to_string(), "更新".to_string(), candidates, 1);

    assert!(candidate_session.is_active());
    assert_eq!(candidate_session.selected_index, 0);

    // Navigation: Down arrow wraps
    candidate_session.next();
    assert_eq!(candidate_session.selected_index, 1);
    candidate_session.next();
    assert_eq!(candidate_session.selected_index, 2);

    // Digit key direct selection: '4' selects Index 3 ("⇒", annotated with "記号")
    let selected_item = candidate_session.select_digit('4').expect("item 4 exists");
    assert_eq!(selected_item.text, "⇒");
    assert_eq!(selected_item.annotation.as_deref(), Some("記号"));

    // Confirm: pure text without annotation is extracted
    let confirmed_text = candidate_session.confirm().expect("confirmed text");
    assert_eq!(confirmed_text, "⇒");
    assert!(!candidate_session.is_active());

    // Replace text in store and verify 1-step undo
    let new_store_text: Vec<u16> = format!("prefix {} suffix", confirmed_text)
        .encode_utf16()
        .collect();
    // Record current text in undo before replacing
    store.get().undo.borrow_mut().push((
        store.get().text.borrow().clone(),
        store.get().selection.get(),
    ));
    *store.get().text.borrow_mut() = new_store_text;
    assert_eq!(
        *store.get().text.borrow(),
        "prefix ⇒ suffix".encode_utf16().collect::<Vec<_>>()
    );

    // 1-step undo restores to before candidate replacement
    store.get().undo_once()?;
    assert_eq!(
        *store.get().text.borrow(),
        "prefix nihongo suffix".encode_utf16().collect::<Vec<_>>()
    );
    println!("Scenario 8: Candidate session navigation, pure confirmation and 1-step Undo: OK");

    // =========================================================================
    // Scenario 9: Decisions Auto-Conversion (ADR 0102)
    // Separate consent, 200ms fixed window, cost protection, and threshold check
    // =========================================================================
    // 1. Separate consent check: unconsented auto conversion fails closed
    assert!(!authority.is_auto_consented());
    let budget = sumibi_host::decisions::BudgetManager::new_in_memory(10_000);
    let mut decisions_buffer = sumibi_core::decisions::DecisionsBuffer::new();

    let state_id = decisions_buffer.push_state("nihon", 1000).expect("pushed");
    assert_eq!(decisions_buffer.pending_count(), 1);

    // 2. 200ms fixed window debounce
    assert!(!decisions_buffer.is_window_elapsed(1150));
    assert!(decisions_buffer.is_window_elapsed(1200));

    let (gen, batch) = decisions_buffer.drain_batch(1200);
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].id, state_id);

    // 3. Independent grant of auto consent
    // When credential is missing, auto consent must fail
    assert!(authority.grant_auto_consent(&destination).is_err());

    // Restore credential and grant independent auto consent
    authority
        .replace_credential(Credential::new("session-key-test".into()))
        .map_err(|_| "credential restore")?;
    authority
        .grant_auto_consent(&destination)
        .map_err(|_| "grant auto consent")?;
    assert!(authority.is_auto_consented());

    // 4. Cost protection pre-allocation
    assert_eq!(budget.consumed(), 0);
    budget.reserve().map_err(|_| "reserve budget")?;
    assert_eq!(budget.consumed(), 1);

    // 5. Strict schema & threshold evaluation
    // Confidence >= 0.5 and should_convert: true is accepted
    let item_convert = sumibi_core::decisions::DecisionResultItem {
        id: state_id,
        should_convert: true,
        confidence: 0.95,
        converted_text: Some("日本".to_string()),
    };
    let replacement = decisions_buffer
        .evaluate_item(gen, &item_convert)
        .expect("replacement accepted");
    assert_eq!(replacement.id, state_id);
    assert_eq!(replacement.converted_text, "日本");

    // Confidence < 0.5 is rejected
    let item_low = sumibi_core::decisions::DecisionResultItem {
        id: state_id,
        should_convert: true,
        confidence: 0.45,
        converted_text: Some("日本".to_string()),
    };
    assert!(decisions_buffer.evaluate_item(gen, &item_low).is_none());

    // 6. Manual conversion interrupts & invalidates pending auto decisions
    decisions_buffer.push_state("auto_pending", 2000);
    assert_eq!(decisions_buffer.pending_count(), 1);
    decisions_buffer.invalidate_manual_conversion();
    assert_eq!(decisions_buffer.pending_count(), 0);

    // Older generation result is rejected after manual invalidation
    assert!(decisions_buffer.evaluate_item(gen, &item_convert).is_none());

    // 7. Revoke auto consent
    authority
        .revoke_auto_consent()
        .map_err(|_| "revoke auto consent")?;
    assert!(!authority.is_auto_consented());

    println!("Scenario 9: Decisions auto-conversion (separate consent, fixed window, cost protection): OK");

    // =========================================================================
    // Scenario 10: Teardown and Cleanup
    // =========================================================================
    server_running.store(false, Ordering::Relaxed);
    association.close()?;
    unsafe {
        manager.SetFocus(None)?;
    }
    pump_tsf_messages(&messages)?;
    service.stop().map_err(|e| format!("service stop: {e:?}"))?;
    println!("Scenario 10: Teardown and cleanup: OK");

    Ok(())
}

pub(crate) fn exercise_live_conversion(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Failure> {
    let messages: ITfMessagePump = manager.cast()?;

    // 1. Setup window, TextStore, document and context
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

    println!("Connecting to active Sumibi Host...");
    // 2. Wait for TipHostSession to discover running Host and become ready
    let ready_deadline = Instant::now() + Duration::from_secs(10);
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
        std::thread::sleep(Duration::from_millis(100));
    }
    if !key_admitted {
        return Err("timed out waiting for active Host connection. Is sumibi-host running with consent granted?".into());
    }
    println!("Hot-key (Ctrl+J) admitted by Host! Waiting for OpenAI API response...");

    // 3. Wait for OpenAI API conversion result via DrainWindow
    let drain_deadline = Instant::now() + Duration::from_secs(20);
    let initial_text: Vec<u16> = "prefix nihongo suffix".encode_utf16().collect();
    let mut replaced_text = String::new();
    let mut replaced = false;
    while Instant::now() < drain_deadline {
        pump_tsf_messages(&messages)?;
        let current_u16 = store.get().text.borrow().clone();
        if current_u16 != initial_text {
            replaced_text = String::from_utf16_lossy(&current_u16);
            replaced = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !replaced {
        return Err("timed out waiting for OpenAI conversion response".into());
    }
    println!(">>> Live Conversion Success! Result: \"{replaced_text}\"");
    assert_eq!(
        store.get().undo.borrow().len(),
        1,
        "undo record must be created"
    );

    // 4. Test 1-step Undo
    println!("Testing 1-step Undo (Ctrl+Z)...");
    store.get().undo_once()?;
    let restored = String::from_utf16_lossy(&store.get().text.borrow());
    println!(">>> Undo Restored: \"{restored}\"");
    assert_eq!(
        restored, "prefix nihongo suffix",
        "text must be restored to original"
    );

    association.close()?;
    unsafe {
        manager.SetFocus(None)?;
    }
    pump_tsf_messages(&messages)?;
    println!("Live API verification completed successfully!");

    Ok(())
}

pub(crate) fn exercise_live_demo(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Failure> {
    let messages: ITfMessagePump = manager.cast()?;
    let window = crate::window_host::Window::new(manager)?;
    window.set_title("Sumibi Live Demo | Ctrl+J: 変換 | Ctrl+Z: 元に戻す | 閉じるで終了")?;

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
    window
        .focus()
        .map_err(|error| format!("window focus: {error}"))?;
    unsafe {
        manager.SetFocus(&document.0)?;
    }

    let _live = crate::live_render::Binding::attach(&window, &context, client)?;

    let provider = unsafe { manager.GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)? };
    let sink: ITfKeyEventSink = unsafe {
        provider
            .GetFunction(
                &sumibi_tip::spike::FUNCTION_TEST_KEY_SINK,
                &ITfKeyEventSink::IID,
            )?
            .cast()?
    };

    println!("Connecting to active Sumibi Host...");
    let ready_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < ready_deadline {
        unsafe {
            manager.SetFocus(&document.0)?;
        }
        pump_tsf_messages(&messages)?;
        std::thread::sleep(Duration::from_millis(50));
    }

    let store_for_undo = store.clone();
    let sink_for_key = sink.clone();
    let context_for_key = context.clone();
    let candidate_session = std::rc::Rc::new(std::cell::RefCell::new(
        None::<sumibi_tip::candidate_session::CandidateSession>,
    ));

    let candidate_for_key = candidate_session.clone();
    let store_for_key = store.clone();
    window.set_input(std::rc::Rc::new(move |msg, wparam, _lparam| {
        if msg == windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN {
            let ctrl_down = unsafe {
                windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(
                    windows::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL.0 as i32,
                ) < 0
            };

            // 1. Ctrl+J: Initial conversion
            if ctrl_down && (wparam.0 == 'J' as usize || wparam.0 == 'j' as usize) {
                println!(">>> Ctrl+J 検知: OpenAI 変換リクエストを送信中...");
                let admitted =
                    unsafe { sink_for_key.OnPreservedKey(&context_for_key, &COMMAND_CONVERT) };
                match admitted {
                    Ok(b) if b.as_bool() => {
                        println!("    リクエスト送信完了。応答を待機中...");
                        // Build integrated candidate list for demonstration
                        let demo_candidates = sumibi_host::local_dictionary::build_complete_candidates(
                            "nihongo",
                            Some("日本語"),
                            &["にほんご".to_string(), "ニホンゴ".to_string()],
                            10,
                        );
                        let session = sumibi_tip::candidate_session::CandidateSession::new(
                            "nihongo".to_string(),
                            "日本語".to_string(),
                            demo_candidates,
                            1,
                        );
                        println!();
                        println!(">>> 候補リスト (数字キー 1-9 で直接確定、上下キーで選択、Enterで確定、Escで取消):");
                        for (idx, item) in session.candidates.iter().enumerate() {
                            let mark = if idx == session.selected_index { ">" } else { " " };
                            let tag = item.annotation.as_deref().unwrap_or(item.source.label());
                            println!("   {mark} [{}] {}  [{tag}]", idx + 1, item.text);
                        }
                        println!();
                        *candidate_for_key.borrow_mut() = Some(session);
                    }
                    Ok(_) => println!("    Host がまだ準備中か拒絶されました。"),
                    Err(e) => println!("    エラー: {e:?}"),
                }
                return Ok(true);
            }

            // 2. Ctrl+Z: 1-step Undo
            if ctrl_down && (wparam.0 == 'Z' as usize || wparam.0 == 'z' as usize) {
                println!(">>> Ctrl+Z 検知: 元に戻す (Undo) を実行しました。");
                let _ = store_for_undo.get().undo_once();
                candidate_for_key.borrow_mut().take();
                return Ok(true);
            }

            // 3. Navigation while candidate session is active
            if let Some(session) = candidate_for_key.borrow_mut().as_mut() {
                if session.is_active() {
                    let vk_down = windows::Win32::UI::Input::KeyboardAndMouse::VK_DOWN.0 as usize;
                    let vk_up = windows::Win32::UI::Input::KeyboardAndMouse::VK_UP.0 as usize;
                    let vk_return = windows::Win32::UI::Input::KeyboardAndMouse::VK_RETURN.0 as usize;
                    let vk_escape = windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE.0 as usize;

                    if wparam.0 == vk_down {
                        session.next();
                        let cur = session.selected_candidate().unwrap();
                        println!("  -> フォーカス: [{}] {} [{}]", session.selected_index + 1, cur.text, cur.source.label());
                        return Ok(true);
                    } else if wparam.0 == vk_up {
                        session.prev();
                        let cur = session.selected_candidate().unwrap();
                        println!("  -> フォーカス: [{}] {} [{}]", session.selected_index + 1, cur.text, cur.source.label());
                        return Ok(true);
                    } else if wparam.0 >= '1' as usize && wparam.0 <= '9' as usize {
                        let digit = char::from_u32(wparam.0 as u32).unwrap();
                        if let Some(item) = session.select_digit(digit) {
                            let text = item.text.clone();
                            println!(">>> 数字キー [{digit}] 選択確定: \"{text}\" で置換します (出所注釈は本文に含みません)");
                            let _ = session.confirm();
                            // Push current state into undo and replace
                            store_for_key.get().undo.borrow_mut().push((
                                store_for_key.get().text.borrow().clone(),
                                store_for_key.get().selection.get(),
                            ));
                            *store_for_key.get().text.borrow_mut() = format!("prefix {text} suffix").encode_utf16().collect();
                            return Ok(true);
                        }
                    } else if wparam.0 == vk_return {
                        if let Some(text) = session.confirm() {
                            println!(">>> Enter 確定: \"{text}\" で置換します");
                            store_for_key.get().undo.borrow_mut().push((
                                store_for_key.get().text.borrow().clone(),
                                store_for_key.get().selection.get(),
                            ));
                            *store_for_key.get().text.borrow_mut() = format!("prefix {text} suffix").encode_utf16().collect();
                            return Ok(true);
                        }
                    } else if wparam.0 == vk_escape {
                        println!(">>> Esc 取消: 候補選択をキャンセルしました。");
                        session.cancel();
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }))?;

    println!();
    println!("===================================================================");
    println!("  Sumibi Live Demo ウィンドウが起動しました！");
    println!("  1. 表示されたウィンドウをクリックしてアクティブにします。");
    println!("  2. キーボードで [Ctrl + J] を押すと変換され、候補リストが表示されます。");
    println!("  3. [1 〜 9] の数字キー または [上下キー + Enter] で候補を選択・確定置換できます。");
    println!("  4. [Ctrl + Z] を押すと、1回で直前の状態に Undo されます。");
    println!("  5. [Esc] で候補選択をキャンセル、ウィンドウの [×] で終了します。");
    println!("===================================================================");
    println!();

    let mut message = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    while window.is_open() {
        if !window.get_message(&mut message)? {
            break;
        }
        window.route_message(&message)?;
    }

    association.close()?;
    unsafe {
        manager.SetFocus(None)?;
    }
    println!("Live Demo 終了しました。");
    Ok(())
}
