//! Generic retained-range manual adapter on real TSF. The ACP fixture remains
//! an ACP fixture: this is not a claim of non-ACP application compatibility.
use super::*;
use std::time::{Duration, Instant};
use sumibi_tip::edit::manual::ManualEdit;

pub(crate) fn exercise_ranges(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    unsafe {
        for (index, case) in [
            "apply",
            "unicode",
            "large-prefix",
            "source-selection",
            "source-direction",
            "source-restored",
            "source-lock",
            "source-policy",
            "source-private",
            "apply-selection",
            "apply-direction",
            "apply-text",
        ]
        .into_iter()
        .enumerate()
        {
            let prefix = if case == "large-prefix" {
                "p".repeat(4096)
            } else {
                "prefix ".into()
            };
            let source = if case == "unicode" {
                "a😀b"
            } else {
                "nihongo"
            };
            let original = format!("{prefix}{source} suffix");
            let window = crate::window_host::Window::new(manager)?;
            let store = ComObject::new(TextStore::new(&original));
            let mut selected = store.get().selection.get();
            selected.acpStart = prefix.encode_utf16().count() as i32;
            selected.acpEnd = selected.acpStart + source.encode_utf16().count() as i32;
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
            let context = context.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
            document.0.Push(&context)?;
            scope_fixture::initialise_context(&context, client)?;
            let mut association =
                crate::window_host::Association::new(manager, &window, &document.0)?;
            manager.SetFocus(&document.0)?;
            pump_tsf_messages()?;
            let policy = Rc::new(Cell::new(EditPolicy {
                allowed: true,
                revision: Revision {
                    input: 1,
                    settings: 1,
                    authorization: 1,
                },
            }));
            let id = sumibi_core::RequestId(index as u128 + 1);
            let ticket = ManualEdit::capture(
                manager,
                &context,
                client,
                policy.clone(),
                id,
                index as u128 + 1,
                Instant::now() + Duration::from_secs(5),
            )?;
            let before = sumibi_tip::spike::SumibiSpikeBodyReadCount();
            let mut changed = selected;
            if case == "source-direction" || case == "apply-direction" {
                changed.style.ase = TS_AE_START;
            } else if case == "source-restored" {
                changed.acpEnd -= 1;
            } else {
                changed.acpEnd = changed.acpStart;
            }
            if case.starts_with("source-") {
                match case {
                    "source-lock" => store.get().deny_lock.set(true),
                    "source-policy" => {
                        let mut p = policy.get();
                        p.allowed = false;
                        policy.set(p);
                    }
                    "source-private" => {
                        *store.get().scopes.borrow_mut() = ScopeSpec::List(vec![IS_PRIVATE])
                    }
                    _ => store.get().selection.set(changed), // deliberately no notification
                }
                assert!(
                    ticket.source().is_err(),
                    "{case}: changed source must be denied"
                );
                assert_eq!(
                    sumibi_tip::spike::SumibiSpikeBodyReadCount(),
                    before,
                    "{case}: metadata only"
                );
                store.get().selection.set(selected);
                store.get().deny_lock.set(false);
                let mut p = policy.get();
                p.allowed = true;
                policy.set(p);
                *store.get().scopes.borrow_mut() = ScopeSpec::List(vec![IS_TEXT]);
                assert!(
                    ticket.source().is_err(),
                    "{case}: observed failure cannot revive"
                );
                assert!(ticket
                    .finish(manager, client, id, policy.get().revision, "日本語".into())
                    .is_err());
                assert_eq!(sumibi_tip::spike::SumibiSpikeBodyReadCount(), before);
                assert_eq!(store.get().set_text_calls.get(), 0);
                assert!(store.get().undo.borrow().is_empty());
            } else {
                assert_eq!(ticket.source()?, source);
                assert_eq!(ticket.source()?, source);
                assert_eq!(
                    sumibi_tip::spike::SumibiSpikeBodyReadCount(),
                    before,
                    "source must not reread text"
                );
                if case.starts_with("apply-") {
                    if case == "apply-text" {
                        // TSF caches text. Host-originated body changes require
                        // the real OnTextChange contract; raw unnotified memory
                        // mutations cannot be used to claim cache invalidation.
                        store.get().external_text(
                            format!("{prefix}NIHONGO suffix").encode_utf16().collect(),
                        )?;
                    } else {
                        store.get().selection.set(changed);
                    }
                    assert!(ticket
                        .finish(manager, client, id, policy.get().revision, "日本語".into())
                        .is_err());
                    if case != "apply-text" {
                        assert_eq!(sumibi_tip::spike::SumibiSpikeBodyReadCount(), before);
                    }
                    assert_eq!(store.get().set_text_calls.get(), 0);
                    assert!(store.get().undo.borrow().is_empty());
                    if case == "apply-text" {
                        store
                            .get()
                            .external_text(original.encode_utf16().collect())?;
                    }
                } else {
                    ticket.finish(manager, client, id, policy.get().revision, "日本語".into())?;
                    assert_eq!(
                        *store.get().text.borrow(),
                        format!("{prefix}日本語 suffix")
                            .encode_utf16()
                            .collect::<Vec<_>>()
                    );
                    assert_eq!(store.get().set_text_calls.get(), 1);
                    assert_eq!(store.get().undo.borrow().len(), 1);
                    store.get().undo_once()?;
                }
            }
            assert_eq!(
                *store.get().text.borrow(),
                original.encode_utf16().collect::<Vec<_>>()
            );
            association.close()?;
            manager.SetFocus(None)?;
            println!("Retained TSF range / {case}: OK");
        }
    }
    Ok(())
}
