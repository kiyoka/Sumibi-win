//! Dedicated in-memory text store for real TSF edit-session verification.
//! Never connected to user documents or installed as an input application.
#![allow(non_snake_case)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use sumibi_core::Revision;
use sumibi_tip::edit::{capture_selection, replace_selection, EditPolicy, MAX_SELECTION_UNITS};
use sumibi_tip::spike::ISpikeBroker;
use windows::core::{
    implement, ComObject, IUnknown, Interface, Ref, Result, BOOL, GUID, HRESULT, PCWSTR, PWSTR,
};
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOTIMPL, HWND, POINT, RECT};
use windows::Win32::System::Com::{IDataObject, FORMATETC};
use windows::Win32::UI::TextServices::*;

#[path = "input_host.rs"]
mod input_host;
pub(super) use input_host::{
    exercise_broker_input, exercise_close, exercise_input, exercise_tracking, run_broker_demo,
    run_demo, run_tracking_demo,
};
#[path = "delayed_host.rs"]
mod delayed_host;
pub(super) use delayed_host::exercise_delayed;
#[path = "broker_host.rs"]
mod broker_host;
pub(super) use broker_host::exercise_broker;
#[path = "broker_keys.rs"]
mod broker_keys;
pub(super) use broker_keys::exercise_broker_keys;
#[path = "scope_fixture.rs"]
mod scope_fixture;
use scope_fixture::{scope_value, ScopeSpec};
#[path = "protection_host.rs"]
mod protection_host;
pub(super) use protection_host::exercise_protection;
#[path = "composition_host.rs"]
mod composition_host;
pub(super) use composition_host::exercise_composition;
#[path = "owned_composition_host.rs"]
mod owned_composition_host;
pub(super) use owned_composition_host::exercise_owned_composition;

type EditHistory = RefCell<Vec<(Vec<u16>, TS_SELECTION_ACP)>>;

#[implement(ITextStoreACP, ITfContextOwnerCompositionSink)]
struct TextStore {
    text: RefCell<Vec<u16>>,
    selection: Cell<TS_SELECTION_ACP>,
    sink: RefCell<Option<ITextStoreACPSink>>,
    lock: Cell<u32>,
    readonly: Cell<bool>,
    deny_lock: Cell<bool>,
    skip_callback: Cell<bool>,
    read_limit: Cell<usize>,
    reads: Cell<usize>,
    body_reads: Cell<usize>,
    undo: EditHistory,
    redo: EditHistory,
    window: Cell<HWND>,
    scopes: Rc<RefCell<ScopeSpec>>,
    scope_queries: Rc<Cell<usize>>,
    scope_requested: Cell<bool>,
    static_status: Cell<u32>,
    dynamic_status: Cell<u32>,
    status_failure: Cell<bool>,
    protect_on_read: Cell<bool>,
    composition_broker: RefCell<Option<ISpikeBroker>>,
    composition_events: Cell<u32>,
    reject_composition: Cell<bool>,
}

struct Lock<'a>(&'a Cell<u32>);
impl Drop for Lock<'_> {
    fn drop(&mut self) {
        self.0.set(0);
    }
}

impl TextStore_Impl {
    fn composition_changed(&self) {
        self.composition_events
            .set(self.composition_events.get().saturating_add(1));
        let broker = self.composition_broker.borrow().clone();
        if let Some(broker) = broker {
            // The owning host forwards metadata-only invalidation, including
            // empty compositions that do not emit OnEndEdit. Cancel retires the
            // DLL ticket before signaling EXE work; it never waits or edits.
            // Never reject/terminate another composition if cancellation fails.
            unsafe {
                let _ = broker.Cancel(0);
            }
        }
    }
}
impl ITfContextOwnerCompositionSink_Impl for TextStore_Impl {
    fn OnStartComposition(&self, _: Ref<ITfCompositionView>) -> Result<BOOL> {
        self.composition_changed();
        Ok((!self.reject_composition.get()).into())
    }
    fn OnUpdateComposition(&self, _: Ref<ITfCompositionView>, _: Ref<ITfRange>) -> Result<()> {
        self.composition_changed();
        Ok(())
    }
    fn OnEndComposition(&self, _: Ref<ITfCompositionView>) -> Result<()> {
        self.composition_changed();
        Ok(())
    }
}

impl TextStore {
    fn new(text: &str) -> Self {
        let text: Vec<u16> = text.encode_utf16().collect();
        let selection = TS_SELECTION_ACP {
            acpStart: 0,
            acpEnd: text.len() as i32,
            style: TS_SELECTIONSTYLE {
                ase: TS_AE_END,
                fInterimChar: false.into(),
            },
        };
        Self {
            text: RefCell::new(text),
            selection: Cell::new(selection),
            sink: RefCell::new(None),
            lock: Cell::new(0),
            readonly: Cell::new(false),
            deny_lock: Cell::new(false),
            skip_callback: Cell::new(false),
            read_limit: Cell::new(usize::MAX),
            reads: Cell::new(0),
            body_reads: Cell::new(0),
            undo: RefCell::new(Vec::new()),
            redo: RefCell::new(Vec::new()),
            window: Cell::new(HWND::default()),
            scopes: Rc::new(RefCell::new(ScopeSpec::List(vec![IS_TEXT]))),
            scope_queries: Rc::new(Cell::new(0)),
            scope_requested: Cell::new(false),
            static_status: Cell::new(TS_SS_NOHIDDENTEXT),
            dynamic_status: Cell::new(0),
            status_failure: Cell::new(false),
            protect_on_read: Cell::new(false),
            composition_broker: RefCell::new(None),
            composition_events: Cell::new(0),
            reject_composition: Cell::new(false),
        }
    }
    fn readable(&self) -> Result<()> {
        if self.lock.get() & TS_LF_READ.0 == 0 {
            Err(TS_E_NOLOCK.into())
        } else {
            Ok(())
        }
    }
    fn writable(&self) -> Result<()> {
        if self.readonly.get() {
            return Err(TS_E_READONLY.into());
        }
        if self.lock.get() & TS_LF_READWRITE.0 != TS_LF_READWRITE.0 {
            Err(TS_E_NOLOCK.into())
        } else {
            Ok(())
        }
    }
    fn bounds(&self, start: i32, end: i32) -> Result<(usize, usize)> {
        let len = self.text.borrow().len();
        let end = if end == -1 { len as i32 } else { end };
        if start < 0 || end < start || end as usize > len {
            return Err(TS_E_INVALIDPOS.into());
        }
        Ok((start as usize, end as usize))
    }
    fn undo_once(&self) -> Result<()> {
        self.restore_history(&self.undo, &self.redo)
    }
    fn redo_once(&self) -> Result<()> {
        self.restore_history(&self.redo, &self.undo)
    }
    fn restore_history(&self, source: &EditHistory, destination: &EditHistory) -> Result<()> {
        if self.readonly.get() {
            return Err(TS_E_READONLY.into());
        }
        if self.lock.get() != 0 {
            return Err(TS_E_NOLOCK.into());
        }
        let (text, selection) = source.borrow_mut().pop().ok_or(E_FAIL)?;
        let previous = self.text.replace(text);
        let old_length = previous.len() as i32;
        destination
            .borrow_mut()
            .push((previous, self.selection.get()));
        self.selection.set(selection);
        let new_length = self.text.borrow().len() as i32;
        let sink = self.sink.borrow().clone();
        if let Some(sink) = sink {
            // SAFETY: restore body and selection together before notifying TSF,
            // outside a lock and without holding any RefCell borrow.
            unsafe {
                sink.OnTextChange(
                    TS_ST_NONE,
                    &TS_TEXTCHANGE {
                        acpStart: 0,
                        acpOldEnd: old_length,
                        acpNewEnd: new_length,
                    },
                )?;
                sink.OnSelectionChange()?;
            }
        }
        Ok(())
    }
    fn external_text(&self, text: Vec<u16>) -> Result<()> {
        if self.lock.get() != 0 {
            return Err(TS_E_NOLOCK.into());
        }
        let old = self.text.borrow().len() as i32;
        let new = text.len() as i32;
        *self.text.borrow_mut() = text;
        self.redo.borrow_mut().clear();
        let sink = self.sink.borrow().clone();
        if let Some(sink) = sink {
            // SAFETY: notify the real TSF manager after releasing the data borrow.
            unsafe {
                sink.OnTextChange(
                    TS_ST_NONE,
                    &TS_TEXTCHANGE {
                        acpStart: 0,
                        acpOldEnd: old,
                        acpNewEnd: new,
                    },
                )?
            };
        }
        Ok(())
    }
}

impl ITextStoreACP_Impl for TextStore_Impl {
    fn AdviseSink(&self, iid: *const GUID, sink: Ref<IUnknown>, _: u32) -> Result<()> {
        if iid.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: COM supplies a valid IID pointer for the duration of this call.
        if unsafe { *iid } != ITextStoreACPSink::IID {
            return Err(E_INVALIDARG.into());
        }
        let sink = sink.ok()?.cast::<ITextStoreACPSink>()?;
        if let Some(existing) = self.sink.borrow().as_ref() {
            if existing != &sink {
                return Err(E_FAIL.into());
            }
        }
        *self.sink.borrow_mut() = Some(sink);
        Ok(())
    }
    fn UnadviseSink(&self, sink: Ref<IUnknown>) -> Result<()> {
        let supplied = sink.ok()?.cast::<ITextStoreACPSink>()?;
        if self.sink.borrow().as_ref() != Some(&supplied) {
            return Err(E_FAIL.into());
        }
        self.sink.borrow_mut().take();
        Ok(())
    }
    fn RequestLock(&self, flags: u32) -> Result<HRESULT> {
        if self.skip_callback.get() {
            return Ok(windows::Win32::Foundation::S_OK);
        }
        if self.deny_lock.get() || self.lock.get() != 0 {
            return Ok(TS_E_SYNCHRONOUS);
        }
        if self.readonly.get() && flags & TS_LF_READWRITE.0 == TS_LF_READWRITE.0 {
            return Ok(TS_E_READONLY);
        }
        let sink = self.sink.borrow().clone().ok_or(E_FAIL)?;
        self.lock.set(flags);
        let _lock = Lock(&self.lock);
        // SAFETY: live sink reference on the owning thread. Keep no RefCell
        // borrow across OnLockGranted, which calls back into this text store.
        Ok(unsafe { (sink.vtable().OnLockGranted)(sink.as_raw(), TEXT_STORE_LOCK_FLAGS(flags)) })
    }
    fn GetStatus(&self) -> Result<TS_STATUS> {
        if self.status_failure.get() {
            return Err(E_FAIL.into());
        }
        Ok(TS_STATUS {
            dwDynamicFlags: self.dynamic_status.get()
                | if self.readonly.get() {
                    TS_SD_READONLY
                } else {
                    0
                },
            dwStaticFlags: self.static_status.get(),
        })
    }
    fn QueryInsert(
        &self,
        start: i32,
        end: i32,
        _: u32,
        out_start: *mut i32,
        out_end: *mut i32,
    ) -> Result<()> {
        self.bounds(start, end)?;
        if out_start.is_null() || out_end.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: caller supplies writable output slots.
        unsafe {
            out_start.write(start);
            out_end.write(end);
        }
        Ok(())
    }
    fn GetSelection(
        &self,
        index: u32,
        count: u32,
        output: *mut TS_SELECTION_ACP,
        fetched: *mut u32,
    ) -> Result<()> {
        self.readable()?;
        if output.is_null()
            || fetched.is_null()
            || count == 0
            || (index != 0 && index != TF_DEFAULT_SELECTION)
        {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: checked caller-owned output slots for one selection.
        unsafe {
            output.write(self.selection.get());
            fetched.write(1);
        }
        Ok(())
    }
    fn SetSelection(&self, count: u32, input: *const TS_SELECTION_ACP) -> Result<()> {
        self.writable()?;
        if count != 1 || input.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: COM supplies one readable selection.
        let input = unsafe { *input };
        self.bounds(input.acpStart, input.acpEnd)?;
        self.selection.set(input);
        Ok(())
    }
    fn GetText(
        &self,
        start: i32,
        end: i32,
        output: PWSTR,
        capacity: u32,
        copied: *mut u32,
        runs: *mut TS_RUNINFO,
        run_capacity: u32,
        run_count: *mut u32,
        next: *mut i32,
    ) -> Result<()> {
        self.readable()?;
        self.reads.set(self.reads.get() + 1);
        let (start, end) = self.bounds(start, end)?;
        if copied.is_null()
            || run_count.is_null()
            || next.is_null()
            || (capacity > 0 && output.is_null())
            || (run_capacity > 0 && runs.is_null())
        {
            return Err(E_INVALIDARG.into());
        }
        let requested = if capacity > 0 {
            capacity as usize
        } else if run_capacity > 0 {
            end - start
        } else {
            0
        };
        let count = (end - start).min(requested).min(self.read_limit.get());
        // TSF can call GetText only for run metadata (capacity == 0). Count
        // actual UTF-16 body copies separately for the privacy boundary tests.
        if capacity > 0 && count > 0 {
            self.body_reads.set(self.body_reads.get() + 1);
            if self.protect_on_read.replace(false) {
                *self.scopes.borrow_mut() = ScopeSpec::List(vec![IS_PRIVATE]);
            }
        }
        let text = self.text.borrow();
        // SAFETY: bounded caller-owned buffers; text is borrowed only for copy.
        unsafe {
            if count > 0 && capacity > 0 {
                std::ptr::copy_nonoverlapping(text[start..].as_ptr(), output.0, count);
            }
            copied.write(if capacity > 0 { count as u32 } else { 0 });
            next.write((start + count) as i32);
            run_count.write(if run_capacity > 0 && count > 0 { 1 } else { 0 });
            if run_capacity > 0 && count > 0 {
                runs.write(TS_RUNINFO {
                    uCount: count as u32,
                    r#type: TS_RT_PLAIN,
                });
            }
        }
        Ok(())
    }
    fn SetText(
        &self,
        _: u32,
        start: i32,
        end: i32,
        input: &PCWSTR,
        count: u32,
    ) -> Result<TS_TEXTCHANGE> {
        self.writable()?;
        let (start, end) = self.bounds(start, end)?;
        if count > 0 && input.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: COM caller provides count readable UTF-16 units; empty input
        // does not require forming a slice from a null pointer.
        let input = if count == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(input.0, count as usize) }
        };
        self.undo
            .borrow_mut()
            .push((self.text.borrow().clone(), self.selection.get()));
        self.redo.borrow_mut().clear();
        self.text
            .borrow_mut()
            .splice(start..end, input.iter().copied());
        let mut selection = self.selection.get();
        selection.acpStart = start as i32;
        selection.acpEnd = (start + input.len()) as i32;
        self.selection.set(selection);
        Ok(TS_TEXTCHANGE {
            acpStart: start as i32,
            acpOldEnd: end as i32,
            acpNewEnd: (start + input.len()) as i32,
        })
    }
    fn GetFormattedText(&self, _: i32, _: i32) -> Result<IDataObject> {
        Err(E_NOTIMPL.into())
    }
    fn GetEmbedded(&self, _: i32, _: *const GUID, _: *const GUID) -> Result<IUnknown> {
        Err(TS_E_NOOBJECT.into())
    }
    fn QueryInsertEmbedded(&self, _: *const GUID, _: *const FORMATETC) -> Result<BOOL> {
        Ok(false.into())
    }
    fn InsertEmbedded(&self, _: u32, _: i32, _: i32, _: Ref<IDataObject>) -> Result<TS_TEXTCHANGE> {
        Err(E_NOTIMPL.into())
    }
    fn InsertTextAtSelection(
        &self,
        _: u32,
        _: &PCWSTR,
        _: u32,
        _: *mut i32,
        _: *mut i32,
        _: *mut TS_TEXTCHANGE,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn InsertEmbeddedAtSelection(
        &self,
        _: u32,
        _: Ref<IDataObject>,
        _: *mut i32,
        _: *mut i32,
        _: *mut TS_TEXTCHANGE,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn RequestSupportedAttrs(&self, _: u32, count: u32, attrs: *const GUID) -> Result<()> {
        self.request_scope(count, attrs)
    }
    fn RequestAttrsAtPosition(&self, _: i32, count: u32, attrs: *const GUID, _: u32) -> Result<()> {
        self.request_scope(count, attrs)
    }
    fn RequestAttrsTransitioningAtPosition(
        &self,
        _: i32,
        _: u32,
        _: *const GUID,
        _: u32,
    ) -> Result<()> {
        Ok(())
    }
    fn FindNextAttrTransition(
        &self,
        _: i32,
        halt: i32,
        _: u32,
        _: *const GUID,
        _: u32,
        next: *mut i32,
        found: *mut BOOL,
        offset: *mut i32,
    ) -> Result<()> {
        if next.is_null() || found.is_null() || offset.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: caller-owned output slots, no attribute transitions in fixture.
        unsafe {
            next.write(halt);
            found.write(false.into());
            offset.write(0);
        }
        Ok(())
    }
    fn RetrieveRequestedAttrs(
        &self,
        count: u32,
        output: *mut TS_ATTRVAL,
        fetched: *mut u32,
    ) -> Result<()> {
        if fetched.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: TSF owns the returned variant and releases its interface.
        unsafe {
            fetched.write(0);
            if self.scope_requested.replace(false) && count > 0 {
                if output.is_null() {
                    return Err(E_INVALIDARG.into());
                }
                if let Some(value) = scope_value(&self.scopes, &self.scope_queries)? {
                    output.write(TS_ATTRVAL {
                        idAttr: GUID_PROP_INPUTSCOPE,
                        dwOverlapId: 0,
                        varValue: value,
                    });
                    fetched.write(1);
                }
            }
        }
        Ok(())
    }
    fn GetEndACP(&self) -> Result<i32> {
        self.readable()?;
        Ok(self.text.borrow().len() as i32)
    }
    fn GetActiveView(&self) -> Result<u32> {
        Ok(1)
    }
    fn GetACPFromPoint(&self, _: u32, _: *const POINT, _: u32) -> Result<i32> {
        Err(TS_E_NOLAYOUT.into())
    }
    fn GetTextExt(&self, _: u32, _: i32, _: i32, _: *mut RECT, _: *mut BOOL) -> Result<()> {
        Err(TS_E_NOLAYOUT.into())
    }
    fn GetScreenExt(&self, _: u32) -> Result<RECT> {
        Err(TS_E_NOLAYOUT.into())
    }
    fn GetWnd(&self, view: u32) -> Result<HWND> {
        if view != 1 {
            return Err(E_INVALIDARG.into());
        }
        Ok(self.window.get())
    }
}

struct Document(ITfDocumentMgr);
impl Drop for Document {
    fn drop(&mut self) {
        // SAFETY: pop all fixture contexts before releasing its text store.
        let _ = unsafe { self.0.Pop(TF_POPF_ALL) };
    }
}

pub(super) fn exercise(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    // SAFETY: caller initialized COM/TSF on this thread; all documents below
    // contain only fixed synthetic text and are removed before returning.
    unsafe {
        for case in [
            "replace",
            "subrange",
            "same-length-change",
            "selection-change",
            "focus-change",
            "input-generation",
            "settings-revision",
            "authorization-revision",
            "revoked",
            "protected",
            "readonly",
            "read-lock-denied",
            "write-lock-denied",
            "missing-write-callback",
            "no-progress",
            "invalid-utf16",
            "over-limit",
            "limit",
            "unicode",
            "unicode-partial",
            "partial-read",
            "split-surrogate",
            "empty",
        ] {
            let initial = match case {
                "limit" => "a".repeat(MAX_SELECTION_UNITS),
                "subrange" => "prefix nihongo suffix".to_owned(),
                "over-limit" => "a".repeat(MAX_SELECTION_UNITS + 1),
                "unicode" | "unicode-partial" | "split-surrogate" => "a😀e\u{301}".to_owned(),
                "empty" => String::new(),
                _ => "nihongo".to_owned(),
            };
            let store = ComObject::new(TextStore::new(&initial));
            if case == "subrange" {
                let mut selection = store.get().selection.get();
                selection.acpStart = 7;
                selection.acpEnd = 14;
                store.get().selection.set(selection);
            }
            if case == "invalid-utf16" {
                *store.get().text.borrow_mut() = vec![0xd800; 7];
            }
            if case == "readonly" {
                store.get().readonly.set(true);
            }
            if case == "read-lock-denied" {
                store.get().deny_lock.set(true);
            }
            if case == "no-progress" {
                store.get().read_limit.set(0);
            }
            if case == "partial-read" {
                store.get().read_limit.set(2);
            }
            if case == "unicode-partial" {
                store.get().read_limit.set(3);
            }
            if case == "split-surrogate" {
                store.get().read_limit.set(1);
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
            let context = context.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
            document.0.Push(&context)?;
            scope_fixture::initialise_context(&context, client)?;
            manager.SetFocus(&document.0)?;
            let policy = Rc::new(Cell::new(EditPolicy {
                revision: Revision {
                    input: 1,
                    settings: 1,
                    authorization: 1,
                },
                allowed: case != "protected",
            }));
            let rejected_capture = matches!(
                case,
                "protected"
                    | "readonly"
                    | "read-lock-denied"
                    | "no-progress"
                    | "invalid-utf16"
                    | "over-limit"
                    | "empty"
                    | "split-surrogate"
                    | "partial-read"
                    | "unicode-partial"
            );
            if rejected_capture {
                let before = store.get().text.borrow().clone();
                assert!(
                    capture_selection(manager, &context, client, policy.clone()).is_err(),
                    "{case}"
                );
                assert_eq!(*store.get().text.borrow(), before, "{case}");
                if case == "protected" {
                    assert_eq!(store.get().reads.get(), 0);
                }
                manager.SetFocus(None)?;
                println!("Edit guard {case}: OK");
                continue;
            }
            let snapshot = capture_selection(manager, &context, client, policy.clone())?;
            let mut other_document = None;
            let mut other_store = None;
            match case {
                "same-length-change" => store
                    .get()
                    .external_text("NIHONGO".encode_utf16().collect())?,
                "selection-change" => {
                    let mut selection = store.get().selection.get();
                    selection.acpEnd = 3;
                    store.get().selection.set(selection);
                    let sink = store.get().sink.borrow().clone().unwrap();
                    sink.OnSelectionChange()?;
                }
                "focus-change" => {
                    let other = Document(manager.CreateDocumentMgr()?);
                    let second_store = ComObject::new(TextStore::new("nihongo"));
                    let mut second_context = None;
                    let mut second_cookie = 0;
                    other.0.CreateContext(
                        client,
                        0,
                        &second_store.to_interface::<IUnknown>(),
                        &mut second_context,
                        &mut second_cookie,
                    )?;
                    let second_context =
                        second_context.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
                    other.0.Push(&second_context)?;
                    manager.SetFocus(&other.0)?;
                    other_store = Some(second_store);
                    other_document = Some(other);
                }
                "write-lock-denied" => store.get().deny_lock.set(true),
                "missing-write-callback" => store.get().skip_callback.set(true),
                "input-generation" | "settings-revision" | "authorization-revision" | "revoked" => {
                    let mut changed = policy.get();
                    match case {
                        "input-generation" => changed.revision.input += 1,
                        "settings-revision" => changed.revision.settings += 1,
                        "authorization-revision" => changed.revision.authorization += 1,
                        _ => changed.allowed = false,
                    }
                    policy.set(changed);
                }
                _ => {}
            }
            let before = store.get().text.borrow().clone();
            let result = replace_selection(manager, client, snapshot, "日本語");
            if matches!(case, "replace" | "subrange" | "limit" | "unicode") {
                result?;
                let expected = if case == "subrange" {
                    "prefix 日本語 suffix"
                } else {
                    "日本語"
                };
                assert_eq!(
                    *store.get().text.borrow(),
                    expected.encode_utf16().collect::<Vec<_>>()
                );
            } else {
                assert!(result.is_err(), "{case}");
                assert_eq!(*store.get().text.borrow(), before, "{case}");
            }
            if let Some(other_store) = other_store.as_ref() {
                assert_eq!(
                    *other_store.get().text.borrow(),
                    "nihongo".encode_utf16().collect::<Vec<_>>()
                );
            }
            println!("Edit guard {case}: OK");
            manager.SetFocus(None)?;
            drop(other_document);
        }
    }
    exercise_attributes(manager, client)?;
    exercise_previews(manager, client)?;
    println!("Real TSF edit-session scenarios: 23 passed, plus 3 attribute scenarios");
    Ok(())
}

fn exercise_previews(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use sumibi_tip::edit::{capture_selection, preview::Preview, EditPolicy};
    // SAFETY: all documents and text are owned by this synthetic test host.
    unsafe {
        for case in [
            "cancel",
            "commit",
            "revoked",
            "selection-change",
            "text-change",
            "focus-change",
            "cleanup-denied",
            "drop",
        ] {
            let store = ComObject::new(TextStore::new("prefix nihongo suffix"));
            let mut selected = store.get().selection.get();
            selected.acpStart = 7;
            selected.acpEnd = 14;
            store.get().selection.set(selected);
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
            manager.SetFocus(&document.0)?;
            let policy = Rc::new(Cell::new(EditPolicy {
                revision: Revision {
                    input: 1,
                    settings: 1,
                    authorization: 1,
                },
                allowed: true,
            }));
            let snapshot = capture_selection(manager, &context, client, policy.clone())?;
            let mut pending = Preview::begin(manager, client, snapshot)?;
            // Separate read session proves the attribute survived begin's lock.
            pending.verify_attribute(true)?;
            assert!(store.get().undo.borrow().is_empty());
            let mut other_document = None;
            match case {
                "commit" => {
                    pending.commit("日本語")?;
                    assert!(pending.commit("再適用").is_err());
                }
                "revoked" => {
                    let mut p = policy.get();
                    p.allowed = false;
                    policy.set(p);
                    assert!(pending.commit("日本語").is_err());
                }
                "selection-change" => {
                    selected.acpEnd = 10;
                    store.get().selection.set(selected);
                    store
                        .get()
                        .sink
                        .borrow()
                        .clone()
                        .unwrap()
                        .OnSelectionChange()?;
                    assert!(pending.commit("日本語").is_err());
                }
                "text-change" => {
                    store
                        .get()
                        .external_text("prefix NIHONGO suffix".encode_utf16().collect())?;
                    assert!(pending.commit("日本語").is_err());
                }
                "focus-change" => {
                    let other = Document(manager.CreateDocumentMgr()?);
                    let other_store = ComObject::new(TextStore::new("another field"));
                    let mut other_context = None;
                    let mut other_cookie = 0;
                    other.0.CreateContext(
                        client,
                        0,
                        &other_store.to_interface::<IUnknown>(),
                        &mut other_context,
                        &mut other_cookie,
                    )?;
                    other.0.Push(
                        &other_context.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?,
                    )?;
                    manager.SetFocus(&other.0)?;
                    other_document = Some(other);
                    assert!(pending.commit("日本語").is_err());
                }
                "cleanup-denied" => {
                    store.get().deny_lock.set(true);
                    assert!(pending.cancel().is_err());
                    store.get().deny_lock.set(false);
                    pending.verify_attribute(true)?;
                }
                _ => {}
            }
            let before_reads = store.get().reads.get();
            if case == "drop" {
                drop(pending);
                // A new preview refuses existing attributes, so successful
                // reacquisition proves the previous Drop removed its mark.
                let snapshot = capture_selection(manager, &context, client, policy.clone())?;
                let mut next = Preview::begin(manager, client, snapshot)?;
                next.cancel()?;
                next.verify_attribute(false)?;
            } else {
                pending.cancel()?;
                pending.cancel()?;
                assert_eq!(store.get().reads.get(), before_reads);
                pending.verify_attribute(false)?;
            }
            let expected = match case {
                "commit" => "prefix 日本語 suffix",
                "text-change" => "prefix NIHONGO suffix",
                _ => "prefix nihongo suffix",
            };
            assert_eq!(
                *store.get().text.borrow(),
                expected.encode_utf16().collect::<Vec<_>>()
            );
            assert_eq!(
                store.get().undo.borrow().len(),
                usize::from(case == "commit")
            );
            manager.SetFocus(None)?;
            println!("Persistent underline {case}: OK");
            drop(other_document);
        }
    }
    println!("Persistent underline scenarios: 8 passed");
    Ok(())
}

fn exercise_attributes(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use sumibi_tip::edit::{capture_selection, probe_underline, EditPolicy};
    // SAFETY: synthetic host contexts on the active TSF apartment only.
    unsafe {
        for case in ["subrange", "revoked", "write-denied"] {
            let store = ComObject::new(TextStore::new("prefix nihongo suffix"));
            let mut selected = store.get().selection.get();
            selected.acpStart = 7;
            selected.acpEnd = 14;
            store.get().selection.set(selected);
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
            manager.SetFocus(&document.0)?;
            let policy = Rc::new(Cell::new(EditPolicy {
                revision: Revision {
                    input: 1,
                    settings: 1,
                    authorization: 1,
                },
                allowed: true,
            }));
            let snapshot = capture_selection(manager, &context, client, policy.clone())?;
            if case == "revoked" {
                let mut revoked = policy.get();
                revoked.allowed = false;
                policy.set(revoked);
            }
            if case == "write-denied" {
                store.get().deny_lock.set(true);
            }
            let before = store.get().text.borrow().clone();
            let result = probe_underline(manager, client, snapshot);
            if case == "subrange" {
                result?;
            } else {
                assert!(result.is_err(), "{case}");
            }
            assert_eq!(*store.get().text.borrow(), before);
            assert!(store.get().undo.borrow().is_empty());
            manager.SetFocus(None)?;
            println!("Attribute apply / inspect / clear {case}: OK");
        }
    }
    Ok(())
}

pub(super) fn exercise_keys(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    exercise_key_host(manager, client, false)
}

pub(super) fn exercise_window_keys(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    exercise_key_host(manager, client, true)?;
    exercise_rendering(manager, client)
}

fn exercise_rendering(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use sumibi_tip::spike::{ISpikeControl, COMMAND_CONVERT, FUNCTION_TEST_CONTROL};
    let directory = std::env::current_dir()?.join("target/window-test");
    std::fs::create_dir_all(&directory)?;
    // SAFETY: dedicated owned text store and native view in the active STA.
    unsafe {
        let window = crate::window_host::Window::new()?;
        let store = ComObject::new(TextStore::new("prefix nihongo suffix"));
        store.get().window.set(window.hwnd());
        let mut selected = store.get().selection.get();
        selected.acpStart = 7;
        selected.acpEnd = 14;
        store.get().selection.set(selected);
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
        let mut association = crate::window_host::Association::new(manager, &window, &document.0)?;
        window.focus()?;
        pump_tsf_messages()?;
        manager.SetFocus(&document.0)?;
        let control: ISpikeControl = manager
            .GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)?
            .GetFunction(&FUNCTION_TEST_CONTROL, &ISpikeControl::IID)?
            .cast()?;
        control.GrantContext(context.as_raw()).ok()?;
        let mut live = crate::live_render::Binding::attach(&window, &context, client)?;
        let last_update = Cell::new(0);
        let last_paint = Cell::new(window.painted());
        let display = |stage: &str,
                       expected: &str,
                       underline: bool|
         -> std::result::Result<(), Box<dyn std::error::Error>> {
            pump_tsf_messages()?;
            assert!(
                live.updates() > last_update.get(),
                "notification must update the cache"
            );
            assert!(
                window.painted() > last_paint.get(),
                "notification must repaint the view"
            );
            last_update.set(live.updates());
            last_paint.set(window.painted());
            let scene = live.scene()?;
            assert_eq!(scene.text, expected.encode_utf16().collect::<Vec<_>>());
            if underline {
                assert_eq!(scene.underlines.len(), 1);
                assert_eq!(scene.underlines[0], 7..14);
            } else {
                assert!(scene.underlines.is_empty());
            }
            let frame = window.frame()?;
            if underline {
                assert_eq!(frame.spans.len(), 1);
                let span = frame.spans[0].clone();
                let dots = frame.ink(span.clone());
                assert!(dots > 0 && dots < span.len());
                assert_eq!(frame.ink(0..span.start), 0);
                assert_eq!(frame.ink(span.end..crate::render::WIDTH), 0);
            } else {
                assert_eq!(frame.ink(0..crate::render::WIDTH), 0);
            }
            frame.save(&directory.join(format!("{stage}.bmp")))?;
            println!("TSF notification -> cached pixels -> WM_PAINT {stage}: OK");
            Ok(())
        };
        display("original", "prefix nihongo suffix", false)?;
        control.BeginPreview().ok()?;
        display("preview", "prefix nihongo suffix", true)?;
        control.CancelPreview().ok()?;
        display("cancelled", "prefix nihongo suffix", false)?;
        assert!(store.get().undo.borrow().is_empty());
        control.BeginPreview().ok()?;
        let keys: ITfKeystrokeMgr = manager.cast()?;
        assert!(keys
            .SimulatePreservedKey(&context, &COMMAND_CONVERT)?
            .as_bool());
        display("converted", "prefix 日本語 suffix", false)?;
        assert_eq!(store.get().undo.borrow().len(), 1);
        store.get().undo_once()?;
        display("undo", "prefix nihongo suffix", false)?;
        let before = live.updates();
        live.request()?;
        live.request()?;
        live.request()?;
        pump_tsf_messages()?;
        assert_eq!(live.updates(), before + 1);
        pump_tsf_messages()?;
        assert_eq!(
            live.updates(),
            before + 1,
            "read/paint must not schedule itself"
        );
        println!("Render requests coalesce; read and paint do not loop: OK");
        control.BeginPreview().ok()?;
        store.get().deny_lock.set(true);
        let before_failed_paint = window.painted();
        pump_tsf_messages()?;
        assert!(live.scene().is_err());
        assert!(
            window.frame().is_err(),
            "failed read must clear stale underline pixels"
        );
        assert!(window.is_blank());
        assert!(window.painted() > before_failed_paint);
        store.get().deny_lock.set(false);
        control.CancelPreview().ok()?;
        display("recovered", "prefix nihongo suffix", false)?;
        println!("Denied render read clears stale pixels; next notification recovers: OK");
        live.request()?;
        let retired_updates = live.updates();
        live.close()?;
        let mut replacement = crate::live_render::Binding::attach(&window, &context, client)?;
        pump_tsf_messages()?;
        assert_eq!(live.updates(), retired_updates);
        assert_eq!(
            replacement.updates(),
            1,
            "retired message must not execute the new callback"
        );
        assert!(replacement.scene()?.underlines.is_empty());
        assert_eq!(window.frame()?.ink(0..crate::render::WIDTH), 0);
        replacement.close()?;
        pump_tsf_messages()?;
        assert!(window.frame().is_err());
        assert!(window.is_blank());
        println!("Rebinding and detach discard stale queued render requests: OK");
        let doomed = crate::window_host::Window::new()?;
        let mut doomed_binding = crate::live_render::Binding::attach(&doomed, &context, client)?;
        drop(doomed);
        let successor = crate::window_host::Window::new()?;
        let mut successor_binding =
            crate::live_render::Binding::attach(&successor, &context, client)?;
        pump_tsf_messages()?;
        assert_eq!(doomed_binding.updates(), 0);
        assert_eq!(successor_binding.updates(), 1);
        assert!(doomed_binding.request().is_err());
        doomed_binding.close()?;
        successor_binding.close()?;
        drop(successor);
        println!("Window closure discards pending reads before a new view is created: OK");
        control.RevokeContext().ok()?;
        association.close()?;
    }
    println!("Native render snapshots: {}", directory.display());
    Ok(())
}

fn exercise_key_host(
    manager: &ITfThreadMgr,
    client: u32,
    windowed: bool,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use sumibi_tip::spike::{
        ISpikeControl, COMMAND_CONVERT, FUNCTION_TEST_CONTROL, FUNCTION_TEST_KEY_SINK,
        FUNCTION_TEST_MANAGER_SINK,
    };
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    // SAFETY: synthetic documents and same-thread TSF interfaces only. This
    // dispatches a TSF command, without injecting physical desktop key events.
    unsafe {
        let window = if windowed {
            Some(crate::window_host::Window::new()?)
        } else {
            None
        };
        let store = ComObject::new(TextStore::new("nihongo"));
        if let Some(window) = &window {
            store.get().window.set(window.hwnd());
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
        let context = context.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
        document.0.Push(&context)?;
        scope_fixture::initialise_context(&context, client)?;
        let mut association = if let Some(window) = &window {
            let association = crate::window_host::Association::new(manager, window, &document.0)?;
            window.focus()?;
            pump_tsf_messages()?;
            Some(association)
        } else {
            None
        };
        manager.SetFocus(&document.0)?;
        let provider = manager.GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)?;
        if let Some(window) = &window {
            assert_eq!(context.GetActiveView()?.GetWnd()?, window.hwnd());
            println!("Native window / TSF document association / GetWnd: OK");
        }
        let control: ISpikeControl = provider
            .GetFunction(&FUNCTION_TEST_CONTROL, &ISpikeControl::IID)?
            .cast()?;
        let callback: ITfKeyEventSink = provider
            .GetFunction(&FUNCTION_TEST_KEY_SINK, &ITfKeyEventSink::IID)?
            .cast()?;
        let focus_callback: ITfThreadMgrEventSink = provider
            .GetFunction(&FUNCTION_TEST_MANAGER_SINK, &ITfThreadMgrEventSink::IID)?
            .cast()?;
        assert!(provider
            .GetFunction(&GUID::zeroed(), &ISpikeControl::IID)
            .is_err());
        assert!(provider
            .GetFunction(&FUNCTION_TEST_CONTROL, &IUnknown::IID)
            .is_err());
        assert!(control.GrantContext(std::ptr::null_mut()).is_err());
        let keys: ITfKeystrokeMgr = manager.cast()?;
        control.GrantContext(context.as_raw()).ok()?;
        control.GrantContext(context.as_raw()).ok()?;
        control.BeginPreview().ok()?;
        control.VerifyPreview().ok()?;
        store.get().deny_lock.set(true);
        assert!(control.CancelPreview().is_err());
        store.get().deny_lock.set(false);
        control.VerifyPreview().ok()?;
        control.CancelPreview().ok()?;
        assert!(control.VerifyPreview().is_err());
        control.BeginPreview().ok()?;
        store.get().deny_lock.set(true);
        assert!(control.RevokeContext().is_err());
        assert!(control.BeginPreview().is_err());
        store.get().deny_lock.set(false);
        control.RevokeContext().ok()?;
        assert!(control.VerifyPreview().is_err());
        control.GrantContext(context.as_raw()).ok()?;
        control.BeginPreview().ok()?;
        control.VerifyPreview().ok()?;
        assert!(store.get().undo.borrow().is_empty());
        println!("DLL persistent underline / cancellation retry / revoke cleanup: OK");
        assert!(keys
            .SimulatePreservedKey(&context, &COMMAND_CONVERT)?
            .as_bool());
        assert_eq!(
            *store.get().text.borrow(),
            "日本語".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(store.get().undo.borrow().len(), 1);
        println!("DLL preserved-key -> guarded replacement: OK");
        for key in [0x0d, 0x1b, 0x08, 0x41] {
            assert!(!callback
                .OnTestKeyDown(&context, WPARAM(key), LPARAM(0))?
                .as_bool());
            assert!(!callback
                .OnKeyDown(&context, WPARAM(key), LPARAM(0))?
                .as_bool());
            assert!(!callback
                .OnTestKeyUp(&context, WPARAM(key), LPARAM(0))?
                .as_bool());
            assert!(!callback
                .OnKeyUp(&context, WPARAM(key), LPARAM(0))?
                .as_bool());
            assert!(!keys.TestKeyDown(WPARAM(key), LPARAM(0))?.as_bool());
            assert!(!keys.KeyDown(WPARAM(key), LPARAM(0))?.as_bool());
            assert!(!keys.TestKeyUp(WPARAM(key), LPARAM(0))?.as_bool());
            assert!(!keys.KeyUp(WPARAM(key), LPARAM(0))?.as_bool());
        }
        assert_eq!(
            *store.get().text.borrow(),
            "日本語".encode_utf16().collect::<Vec<_>>()
        );
        println!("Enter / Esc / Backspace / letter pass-through: OK");
        store.get().undo_once()?;
        assert_eq!(
            *store.get().text.borrow(),
            "nihongo".encode_utf16().collect::<Vec<_>>()
        );
        assert!(store.get().undo.borrow().is_empty());
        println!("Dedicated host one-step undo: OK");
        for key in [0x1b, 0x0d] {
            control.BeginPreview().ok()?;
            assert!(!callback
                .OnTestKeyDown(&context, WPARAM(key), LPARAM(1 << 29))?
                .as_bool());
            let tested = if windowed {
                keys.TestKeyDown(WPARAM(key), LPARAM(0))?
            } else {
                callback.OnTestKeyDown(&context, WPARAM(key), LPARAM(0))?
            };
            if !tested.as_bool() {
                use windows::Win32::UI::Input::KeyboardAndMouse::{
                    GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
                };
                let modified =
                    [VK_CONTROL, VK_MENU, VK_SHIFT].map(|key| GetKeyState(key.0 as i32) < 0);
                let direct = callback
                    .OnTestKeyDown(&context, WPARAM(key), LPARAM(0))?
                    .as_bool();
                let service = keys.GetForeground()? == sumibi_tip::CLSID_SUMIBI_TIP;
                let focused = manager.GetFocus()?.GetTop()?.cast::<IUnknown>()?
                    == context.cast::<IUnknown>()?;
                let active = window.as_ref().is_some_and(|window| {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() == window.hwnd()
                });
                return Err(format!("preview key routing failed: direct={direct}, modifiers={modified:?}, service={service}, context={focused}, active={active}").into());
            }
            control.VerifyPreview().ok()?;
            let handled = if windowed {
                keys.KeyDown(WPARAM(key), LPARAM(0))?
            } else {
                callback.OnKeyDown(&context, WPARAM(key), LPARAM(0))?
            };
            assert_eq!(handled.as_bool(), key == 0x1b);
            assert!(control.VerifyPreview().is_err());
            assert!(!callback
                .OnTestKeyDown(&context, WPARAM(key), LPARAM(0))?
                .as_bool());
            assert_eq!(
                *store.get().text.borrow(),
                "nihongo".encode_utf16().collect::<Vec<_>>()
            );
            assert!(store.get().undo.borrow().is_empty());
            println!(
                "Preview key {key:#x}: {} cancellation / original Enter pass-through OK",
                if windowed {
                    "TSF key manager"
                } else {
                    "DLL callback"
                }
            );
        }
        control.BeginPreview().ok()?;
        store
            .get()
            .external_text("NIHONGO".encode_utf16().collect())?;
        pump_tsf_messages()?;
        assert!(control.VerifyPreview().is_err());
        control.BeginPreview().ok()?;
        control.CancelPreview().ok()?;
        store
            .get()
            .external_text("nihongo".encode_utf16().collect())?;
        pump_tsf_messages()?;
        println!("DLL external text notification cancels preview: OK");
        control.BeginPreview().ok()?;
        let original_selection = store.get().selection.get();
        let mut moved = original_selection;
        moved.acpEnd = 3;
        store.get().selection.set(moved);
        store
            .get()
            .sink
            .borrow()
            .clone()
            .unwrap()
            .OnSelectionChange()?;
        pump_tsf_messages()?;
        assert!(control.VerifyPreview().is_err());
        // A successful new preview also proves old underline cleanup completed.
        control.BeginPreview().ok()?;
        control.CancelPreview().ok()?;
        store.get().selection.set(original_selection);
        store
            .get()
            .sink
            .borrow()
            .clone()
            .unwrap()
            .OnSelectionChange()?;
        pump_tsf_messages()?;
        println!("DLL external selection notification cancels preview: OK");
        // Schedule cleanup, then start a new preview before pumping messages.
        // A callback for the old generation must never erase the new underline.
        control.BeginPreview().ok()?;
        store.get().selection.set(moved);
        store
            .get()
            .sink
            .borrow()
            .clone()
            .unwrap()
            .OnSelectionChange()?;
        control.BeginPreview().ok()?;
        pump_tsf_messages()?;
        control.VerifyPreview().ok()?;
        control.CancelPreview().ok()?;
        store.get().selection.set(original_selection);
        store
            .get()
            .sink
            .borrow()
            .clone()
            .unwrap()
            .OnSelectionChange()?;
        pump_tsf_messages()?;
        println!("Old cleanup does not remove a new preview: OK");
        control.BeginPreview().ok()?;
        store.get().deny_lock.set(true);
        assert!(callback
            .OnTestKeyDown(&context, WPARAM(0x1b), LPARAM(0))?
            .as_bool());
        assert!(callback
            .OnKeyDown(&context, WPARAM(0x1b), LPARAM(0))?
            .as_bool());
        assert!(control.VerifyPreview().is_err());
        assert!(!callback
            .OnTestKeyDown(&context, WPARAM(0x1b), LPARAM(0))?
            .as_bool());
        store.get().deny_lock.set(false);
        pump_tsf_messages()?;
        control.CancelPreview().ok()?;
        assert!(store.get().undo.borrow().is_empty());
        println!("Esc cancels immediately even when cleanup lock is denied: OK");
        store.get().readonly.set(true);
        let sink = store.get().sink.borrow().clone().unwrap();
        sink.OnStatusChange(TS_SD_READONLY)?;
        assert!(!keys
            .SimulatePreservedKey(&context, &COMMAND_CONVERT)?
            .as_bool());
        assert_eq!(
            *store.get().text.borrow(),
            "nihongo".encode_utf16().collect::<Vec<_>>()
        );
        store.get().readonly.set(false);
        sink.OnStatusChange(TS_SD_READONLY)?;
        drop(sink);
        println!("Read-only input is not converted or consumed: OK");
        let other_window = if windowed {
            Some(crate::window_host::Window::new()?)
        } else {
            None
        };
        let other_store = ComObject::new(TextStore::new("nihongo"));
        if let Some(window) = &other_window {
            other_store.get().window.set(window.hwnd());
        }
        let other_document = Document(manager.CreateDocumentMgr()?);
        let mut other_context = None;
        other_document.0.CreateContext(
            client,
            0,
            &other_store.to_interface::<IUnknown>(),
            &mut other_context,
            &mut cookie,
        )?;
        let other_context =
            other_context.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
        other_document.0.Push(&other_context)?;
        let mut other_association = if let Some(window) = &other_window {
            Some(crate::window_host::Association::new(
                manager,
                window,
                &other_document.0,
            )?)
        } else {
            None
        };
        if let Some(window) = &window {
            // Creating the second native window can temporarily move focus
            // and revoke the first grant. Restore the first view explicitly.
            window.focus()?;
            pump_tsf_messages()?;
            control.GrantContext(context.as_raw()).ok()?;
        }
        control.BeginPreview().ok()?;
        assert!(!callback
            .OnTestKeyDown(&other_context, WPARAM(0x1b), LPARAM(0))?
            .as_bool());
        assert!(!callback
            .OnKeyDown(&other_context, WPARAM(0x1b), LPARAM(0))?
            .as_bool());
        control.VerifyPreview().ok()?;
        // Headless documents did not emit OnSetFocus in this environment.
        // Explicitly exercise the DLL's notification callback as well.
        if let Some(window) = &other_window {
            window.focus()?;
        } else {
            manager.SetFocus(&other_document.0)?;
            focus_callback.OnSetFocus(&other_document.0, &document.0)?;
        }
        pump_tsf_messages()?;
        assert!(control.VerifyPreview().is_err());
        assert!(control.BeginPreview().is_err());
        assert_eq!(
            manager.GetFocus()?.GetTop()?.cast::<IUnknown>()?,
            other_context.cast::<IUnknown>()?
        );
        println!(
            "{} cancels preview; other-context keys pass through: OK",
            if windowed {
                "Native window focus notification"
            } else {
                "DLL focus callback"
            }
        );
        assert!(!keys
            .SimulatePreservedKey(&other_context, &COMMAND_CONVERT)?
            .as_bool());
        assert_eq!(
            *other_store.get().text.borrow(),
            "nihongo".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(
            *store.get().text.borrow(),
            "nihongo".encode_utf16().collect::<Vec<_>>()
        );
        println!("Same-text ungranted context rejected: OK");
        control.RevokeContext().ok()?;
        if let Some(association) = &mut other_association {
            association.close()?;
        }
        if let Some(window) = &window {
            window.focus()?;
        }
        manager.SetFocus(&document.0)?;
        let before_reads = store.get().reads.get();
        assert!(!keys
            .SimulatePreservedKey(&context, &COMMAND_CONVERT)?
            .as_bool());
        assert_eq!(store.get().reads.get(), before_reads);
        assert_eq!(
            *store.get().text.borrow(),
            "nihongo".encode_utf16().collect::<Vec<_>>()
        );
        println!("Revoked command does not read or modify input: OK");
        if let Some(association) = &mut association {
            association.close()?;
        }
        manager.SetFocus(None)?;
    }
    Ok(())
}

fn pump_tsf_messages() -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, MSG, PM_REMOVE};
    // SAFETY: dedicated disposable STA process; deliver its own TSF messages.
    // Bound processing to expose a notification loop instead of hanging a test.
    unsafe {
        let mut message = MSG::default();
        for _ in 0..256 {
            if !PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                return Ok(());
            }
            DispatchMessageW(&message);
        }
    }
    Err(E_FAIL.into())
}
