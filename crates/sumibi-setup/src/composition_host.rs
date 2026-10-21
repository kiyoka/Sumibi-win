//! Foreign-composition boundary using real MSCTF composition objects and the
//! loaded DLL. All compositions here belong to the diagnostic host, not Sumibi.
use super::*;
use crate::broker_bridge::{Bridge, Delivery};
use std::time::{Duration, Instant};
use sumibi_ipc::bootstrap::Mode;
use sumibi_tip::edit::{manual::ManualEdit, preview::Preview};
use sumibi_tip::spike::{
    ISpikeBroker, ISpikeBrokerTransport, ISpikeControl, COMMAND_CONVERT, FUNCTION_TEST_BROKER,
    FUNCTION_TEST_CONTROL, FUNCTION_TEST_KEY_SINK,
};
use windows::Win32::Foundation::{E_ABORT, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::UI::WindowsAndMessaging::{
    MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, QS_ALLINPUT,
};

type Failure = Box<dyn std::error::Error>;
type Action = Box<dyn FnOnce(u32) -> Result<()>>;
#[implement(ITfEditSession)]
struct Session {
    action: RefCell<Option<Action>>,
    active: Rc<Cell<bool>>,
    completed: Rc<Cell<bool>>,
}
impl ITfEditSession_Impl for Session_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        if !self.active.get() {
            return Err(E_ABORT.into());
        }
        let result = self.action.borrow_mut().take().ok_or(E_ABORT)?(cookie);
        self.completed.set(true);
        result
    }
}
fn write(context: &ITfContext, client: u32, action: Action) -> Result<()> {
    let active = Rc::new(Cell::new(true));
    let completed = Rc::new(Cell::new(false));
    let session: ITfEditSession = Session {
        action: RefCell::new(Some(action)),
        active: active.clone(),
        completed: completed.clone(),
    }
    .into();
    let result =
        unsafe { context.RequestEditSession(client, &session, TF_ES_SYNC | TF_ES_READWRITE) };
    active.set(false);
    if result? != windows::Win32::Foundation::S_OK || !completed.get() {
        return Err(E_FAIL.into());
    }
    Ok(())
}
#[implement(ITfCompositionSink)]
struct Sink;
impl ITfCompositionSink_Impl for Sink_Impl {
    fn OnCompositionTerminated(&self, _: u32, _: Ref<ITfComposition>) -> Result<()> {
        Ok(())
    }
}
pub(super) struct Composition {
    context: ITfContext,
    client: u32,
    value: Option<ITfComposition>,
}
impl Composition {
    pub(super) fn start(
        context: &ITfContext,
        client: u32,
        start: i32,
        length: i32,
    ) -> Result<Self> {
        let target = context.clone();
        let output = Rc::new(RefCell::new(None));
        let result = output.clone();
        write(
            context,
            client,
            Box::new(move |cookie| {
                unsafe {
                    let range = target.GetStart(cookie)?;
                    range.cast::<ITfRangeACP>()?.SetExtent(start, length)?;
                    let sink: ITfCompositionSink = Sink.into();
                    let service: ITfContextComposition = target.cast()?;
                    *result.borrow_mut() = Some(service.StartComposition(cookie, &range, &sink)?);
                }
                Ok(())
            }),
        )?;
        let value = output.borrow_mut().take().ok_or(E_FAIL)?;
        Ok(Self {
            context: context.clone(),
            client,
            value: Some(value),
        })
    }
    pub(super) fn end(&mut self) -> Result<()> {
        if let Some(value) = self.value.as_ref() {
            let value = value.clone();
            write(
                &self.context,
                self.client,
                Box::new(move |cookie| unsafe { value.EndComposition(cookie) }),
            )?;
            self.value.take();
        }
        Ok(())
    }
}
impl Drop for Composition {
    fn drop(&mut self) {
        let _ = self.end();
    }
}
fn composition_count(context: &ITfContext) -> Result<usize> {
    let service: ITfContextComposition = context.cast()?;
    let enumerator = unsafe { service.EnumCompositions()? };
    for count in 0..=4 {
        let mut view = [None];
        let mut fetched = 0;
        unsafe {
            enumerator.Next(&mut view, &mut fetched)?;
        }
        if fetched == 0 {
            return Ok(count);
        }
        assert_eq!(fetched, 1);
        assert!(view[0].is_some());
    }
    Err(E_FAIL.into())
}
fn policy() -> Rc<Cell<EditPolicy>> {
    Rc::new(Cell::new(EditPolicy {
        allowed: true,
        revision: Revision {
            input: 1,
            settings: 1,
            authorization: 1,
        },
    }))
}

pub(super) fn empty_cycle(context: &ITfContext, client: u32) -> Result<()> {
    Composition::start(context, client, 0, 0)?.end()
}

pub(crate) fn exercise_composition(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Failure> {
    let cases = [
        "idle",
        "active-whole",
        "active-outside",
        "active-empty",
        "active-multiple",
        "ended-before-request",
        "other-context",
        "start-while-waiting",
        "start-end-while-waiting",
        "empty-start-while-waiting",
        "empty-start-end-while-waiting",
        "empty-start-unassisted",
        "source-after-start",
        "source-after-start-end",
        "apply-after-start",
        "preview-after-start",
    ];
    let module = unsafe {
        windows::Win32::System::LibraryLoader::GetModuleHandleW(windows::core::w!(
            "sumibi_tip.dll"
        ))?
    };
    let counter: unsafe extern "system" fn() -> u64 = unsafe {
        std::mem::transmute(
            windows::Win32::System::LibraryLoader::GetProcAddress(
                module,
                windows::core::s!("SumibiSpikeBodyReadCount"),
            )
            .ok_or("missing spike body counter")?,
        )
    };
    unsafe {
        for (index, case) in cases.into_iter().enumerate() {
            let window = crate::window_host::Window::new()?;
            let store = ComObject::new(TextStore::new("nihongo"));
            store.get().window.set(window.hwnd());
            if case == "active-outside" {
                let mut selection = store.get().selection.get();
                selection.acpEnd = 2;
                store.get().selection.set(selection);
            }
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
            // Prepare the second document before capture, so initialization
            // notifications cannot accidentally invalidate the positive case.
            let other = if case == "other-context" {
                Some(scope_fixture::other_document(manager, client)?)
            } else {
                None
            };
            let mut association =
                crate::window_host::Association::new(manager, &window, &document.0)?;
            manager.SetFocus(&document.0)?;
            pump_tsf_messages()?;
            let mut compositions = Vec::new();
            match case {
                "active-whole" | "ended-before-request" => {
                    compositions.push(Composition::start(&context, client, 0, 7)?)
                }
                "active-outside" => compositions.push(Composition::start(&context, client, 4, 3)?),
                "active-empty" => compositions.push(Composition::start(&context, client, 3, 0)?),
                "active-multiple" => {
                    compositions.push(Composition::start(&context, client, 0, 2)?);
                    compositions.push(Composition::start(&context, client, 4, 3)?);
                }
                "other-context" => compositions.push(Composition::start(
                    &other.as_ref().unwrap().0.GetTop()?,
                    client,
                    0,
                    5,
                )?),
                _ => {}
            }
            if case == "ended-before-request" {
                compositions[0].end()?;
            }
            let initial = match case {
                "active-multiple" => 2,
                "active-whole" | "active-outside" | "active-empty" => 1,
                _ => 0,
            };
            assert_eq!(
                composition_count(&context)?,
                initial,
                "{case}: fixture setup"
            );

            if matches!(case, "source-after-start" | "source-after-start-end") {
                let ticket = ManualEdit::capture(
                    manager,
                    &context,
                    client,
                    policy(),
                    sumibi_core::RequestId(index as u128 + 1),
                    index as u128 + 1,
                    Instant::now() + Duration::from_secs(5),
                )?;
                let mut composition = Composition::start(&context, client, 0, 7)?;
                if case == "source-after-start-end" {
                    composition.end()?;
                }
                let reads = store.get().body_reads.get();
                assert!(ticket.source().is_err(), "{case}");
                assert_eq!(
                    reads,
                    store.get().body_reads.get(),
                    "source must not reread text"
                );
                drop(ticket);
                composition.end()?;
            } else if matches!(case, "apply-after-start" | "preview-after-start") {
                let snapshot = capture_selection(manager, &context, client, policy())?;
                let mut composition = Composition::start(&context, client, 0, 7)?;
                let reads = store.get().body_reads.get();
                if case == "apply-after-start" {
                    assert!(replace_selection(manager, client, snapshot, "日本語").is_err());
                } else {
                    assert!(Preview::begin(manager, client, snapshot).is_err());
                }
                assert_eq!(reads, store.get().body_reads.get());
                assert_eq!(
                    composition_count(&context)?,
                    1,
                    "client must not terminate the composition"
                );
                composition.end()?;
            } else {
                let provider = manager.GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)?;
                let control: ISpikeControl = provider
                    .GetFunction(&FUNCTION_TEST_CONTROL, &ISpikeControl::IID)?
                    .cast()?;
                let broker: ISpikeBroker = provider
                    .GetFunction(&FUNCTION_TEST_BROKER, &ISpikeBroker::IID)?
                    .cast()?;
                let keys: ITfKeyEventSink = provider
                    .GetFunction(&FUNCTION_TEST_KEY_SINK, &ITfKeyEventSink::IID)?
                    .cast()?;
                let bridge = Bridge::new(Mode::DiagnosticFixed { delay_ms: 100 })?;
                let transport = bridge.to_interface::<ISpikeBrokerTransport>();
                broker.Attach(context.as_raw(), transport.as_raw()).ok()?;
                // Test the DLL's property observer independently in nonempty
                // cases. Empty transitions require the owning host's callback.
                if case.starts_with("empty-") && case != "empty-start-unassisted" {
                    *store.get().composition_broker.borrow_mut() = Some(broker.clone());
                }
                let reads = counter();
                let eaten = keys.OnPreservedKey(&context, &COMMAND_CONVERT)?.as_bool();
                if initial != 0 {
                    assert!(!eaten, "{case}: key must pass through");
                    assert!(!bridge.get().active(), "{case}: no broker job");
                    assert_eq!(counter(), reads, "{case}: no DLL body read");
                    assert_eq!(
                        composition_count(&context)?,
                        initial,
                        "client must not terminate another composition"
                    );
                } else {
                    assert!(eaten, "{case}");
                    assert!(counter() > reads, "positive case must use DLL reader");
                    let changing = matches!(
                        case,
                        "start-while-waiting"
                            | "start-end-while-waiting"
                            | "empty-start-while-waiting"
                            | "empty-start-end-while-waiting"
                            | "empty-start-unassisted"
                    );
                    if changing {
                        compositions.push(Composition::start(
                            &context,
                            client,
                            0,
                            if case.starts_with("empty-") { 0 } else { 7 },
                        )?);
                        if case.ends_with("start-end-while-waiting") {
                            compositions.last_mut().unwrap().end()?;
                        }
                        pump_tsf_messages()?;
                        let mut pending = 1;
                        broker.Pending(&mut pending).ok()?;
                        if case == "empty-start-unassisted" {
                            assert_ne!(
                                pending, 0,
                                "native empty composition does not notify the text observer"
                            );
                        } else {
                            assert_eq!(
                                pending, 0,
                                "composition notification must cancel before delivery"
                            );
                        }
                        assert!(
                            store.get().composition_events.get() > 0,
                            "real owner callback must run"
                        );
                    }
                    assert_eq!(
                        wait(&bridge, &broker)?,
                        if changing {
                            Delivery::Discarded
                        } else {
                            Delivery::Applied
                        },
                        "{case}"
                    );
                    if !changing {
                        assert_eq!(
                            *store.get().text.borrow(),
                            "日本語".encode_utf16().collect::<Vec<_>>()
                        );
                    }
                }
                bridge.get().shutdown()?;
                control.RevokeContext().ok()?;
                store.get().composition_broker.borrow_mut().take();
            }
            let applied = matches!(case, "idle" | "ended-before-request" | "other-context");
            if !applied {
                assert_eq!(
                    *store.get().text.borrow(),
                    "nihongo".encode_utf16().collect::<Vec<_>>(),
                    "{case}"
                );
            }
            assert_eq!(
                store.get().undo.borrow().len(),
                usize::from(applied),
                "{case}: no extra text edits"
            );
            for composition in &mut compositions {
                composition.end()?;
            }
            assert_eq!(composition_count(&context)?, 0);
            association.close()?;
            manager.SetFocus(None)?;
            println!("Foreign composition / {case}: OK");
        }
    }
    println!("Foreign composition: {} scenarios passed", cases.len());
    Ok(())
}
fn wait(
    bridge: &ComObject<Bridge>,
    broker: &ISpikeBroker,
) -> std::result::Result<Delivery, Failure> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        pump_tsf_messages()?;
        if let Some(delivery) = bridge.get().drain(broker)? {
            return Ok(delivery);
        }
        let handle = bridge
            .get()
            .wait_handle()
            .ok_or("missing composition test request")?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("composition test timed out".into());
        }
        let status = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[handle]),
                remaining.as_millis() as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if status == WAIT_TIMEOUT
            || (status.0 != WAIT_OBJECT_0.0 && status.0 != WAIT_OBJECT_0.0 + 1)
        {
            return Err("composition test wait failed".into());
        }
    }
}
