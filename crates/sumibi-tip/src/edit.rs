//! Guarded selection editing, used by the explicitly granted development host.
//! Callers must classify input protection before granting a policy. Unknown
//! input must remain denied. All objects stay on the owning TSF thread.
#![allow(non_snake_case)]

use crate::com::{boundary, ObjectLease};
use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::thread::{self, ThreadId};
use sumibi_core::Revision;
use windows::core::{implement, IUnknown, Interface, Result};
use windows::Win32::Foundation::{
    E_ABORT, E_ACCESSDENIED, E_FAIL, E_INVALIDARG, RPC_E_WRONG_THREAD, S_OK,
};
use windows::Win32::UI::TextServices::*;

pub const MAX_SELECTION_UNITS: usize = 1024;
#[cfg(feature = "spike")]
static BODY_READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "spike")]
pub(crate) fn body_read_count() -> u64 {
    BODY_READS.load(std::sync::atomic::Ordering::Relaxed)
}

#[path = "manual_edit.rs"]
pub mod manual;

#[cfg(feature = "spike")]
#[path = "preview.rs"]
pub mod preview;

/// An adapter-owned grant, not a substitute for checking TSF input scopes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EditPolicy {
    pub revision: Revision,
    pub allowed: bool,
}

/// Single-use snapshot. No Debug implementation: it contains private input.
pub struct SelectionSnapshot {
    context: ITfContext,
    range: ITfRange,
    style: TF_SELECTIONSTYLE,
    original: Vec<u16>,
    policy: Rc<Cell<EditPolicy>>,
    captured_policy: EditPolicy,
    protection: crate::protection::Stamp,
    owner: ThreadId,
}

type EditAction = Box<dyn FnOnce(u32) -> Result<()>>;

#[implement(ITfEditSession)]
struct Session {
    action: RefCell<Option<EditAction>>,
    owner: ThreadId,
    _lease: ObjectLease,
}

impl ITfEditSession_Impl for Session_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        boundary(|| {
            if self.owner != thread::current().id() {
                return Err(RPC_E_WRONG_THREAD.into());
            }
            let action = self.action.borrow_mut().take().ok_or(E_ABORT)?;
            action(cookie)
        })
    }
}

fn run(context: &ITfContext, client: u32, write: bool, action: EditAction) -> Result<()> {
    if client == 0 {
        return Err(E_INVALIDARG.into());
    }
    let active = Rc::new(Cell::new(true));
    let completed = Rc::new(Cell::new(false));
    let callback_active = active.clone();
    let callback_completed = completed.clone();
    let session: ITfEditSession = Session {
        action: RefCell::new(Some(Box::new(move |cookie| {
            if !callback_active.get() {
                return Err(E_ABORT.into());
            }
            let result = action(cookie);
            callback_completed.set(true);
            result
        }))),
        owner: thread::current().id(),
        _lease: ObjectLease::new(),
    }
    .into();
    let access = if write { TF_ES_READWRITE } else { TF_ES_READ };
    // SAFETY: all references live on this owning thread. Synchronous-only
    // access never queues a callback to edit a later input target.
    let requested = unsafe { context.RequestEditSession(client, &session, TF_ES_SYNC | access) };
    // Reject any late callback even if a broken host retains the session after
    // denying synchronous access. No work may outlive this request.
    active.set(false);
    let status = requested?;
    // Inspect the nested session HRESULT; success of RequestEditSession alone
    // does not mean DoEditSession ran. In particular, do not accept TF_S_ASYNC.
    if status != S_OK {
        return Err(if status.is_err() { status } else { E_FAIL }.into());
    }
    if !completed.get() {
        return Err(E_FAIL.into());
    }
    Ok(())
}

/// Deferred metadata cleanup only. Never queue text replacement or key replay.
#[cfg(feature = "spike")]
pub(crate) fn queue_cleanup(context: &ITfContext, client: u32, action: EditAction) -> Result<()> {
    let session: ITfEditSession = Session {
        action: RefCell::new(Some(action)),
        owner: thread::current().id(),
        _lease: ObjectLease::new(),
    }
    .into();
    // SAFETY: caller supplies a generation-checked, owner-thread cleanup action.
    let status =
        unsafe { context.RequestEditSession(client, &session, TF_ES_ASYNC | TF_ES_READWRITE)? };
    if status == TF_S_ASYNC {
        Ok(())
    } else {
        Err(if status.is_err() { status } else { E_FAIL }.into())
    }
}

fn same_context(left: &ITfContext, right: &ITfContext) -> Result<bool> {
    Ok(left.cast::<IUnknown>()? == right.cast::<IUnknown>()?)
}

fn ensure_target(manager: &ITfThreadMgr, context: &ITfContext) -> Result<()> {
    // SAFETY: read-only COM calls on the owning TSF thread, no borrowed state.
    unsafe {
        if !same_context(&manager.GetFocus()?.GetTop()?, context)? {
            return Err(E_ABORT.into());
        }
        if context.GetStatus()?.dwDynamicFlags & TS_SD_READONLY != 0 {
            return Err(E_ACCESSDENIED.into());
        }
    }
    Ok(())
}

struct RawSelection(TF_SELECTION);
impl Drop for RawSelection {
    fn drop(&mut self) {
        // SAFETY: GetSelection gives us one owned COM reference, even if a
        // failing host partially initializes the output. Option starts as None.
        unsafe { ManuallyDrop::drop(&mut self.0.range) };
    }
}

fn selection(context: &ITfContext, cookie: u32) -> Result<(ITfRange, TF_SELECTIONSTYLE)> {
    let mut selection = RawSelection(TF_SELECTION::default());
    let mut fetched = 0;
    // SAFETY: cookie is valid only inside DoEditSession; output slots are live.
    unsafe {
        context.GetSelection(
            cookie,
            TF_DEFAULT_SELECTION,
            std::slice::from_mut(&mut selection.0),
            &mut fetched,
        )?
    };
    if fetched != 1 || selection.0.style.fInterimChar.as_bool() {
        return Err(E_ABORT.into());
    }
    let range = selection.0.range.take().ok_or(E_FAIL)?;
    Ok((range, selection.0.style))
}

fn read_all(range: &ITfRange, cookie: u32) -> Result<Vec<u16>> {
    // SAFETY: cloned range and edit cookie are confined to this edit session.
    unsafe {
        let context = range.GetContext()?;
        let protection = crate::protection::check(&context, range, cookie)?;
        let cursor = range.Clone()?;
        let mut text = Vec::new();
        while !cursor.IsEmpty(cookie)?.as_bool() {
            crate::protection::validate(&context, range, cookie, protection)?;
            let mut chunk = [0u16; 256];
            let mut count = 0;
            #[cfg(feature = "spike")]
            BODY_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            cursor.GetText(cookie, 0, &mut chunk, &mut count)?;
            crate::protection::validate(&context, range, cookie, protection)?;
            let count = count as usize;
            if count == 0 || count > chunk.len() || text.len() + count > MAX_SELECTION_UNITS {
                return Err(E_ABORT.into());
            }
            text.extend_from_slice(&chunk[..count]);
            let mut shifted = 0;
            cursor.ShiftStart(cookie, count as i32, &mut shifted, std::ptr::null())?;
            if shifted != count as i32 {
                return Err(E_ABORT.into());
            }
        }
        if text.is_empty() || String::from_utf16(&text).is_err() {
            return Err(E_ABORT.into());
        }
        Ok(text)
    }
}

pub fn capture_selection(
    manager: &ITfThreadMgr,
    context: &ITfContext,
    client: u32,
    policy: Rc<Cell<EditPolicy>>,
) -> Result<SelectionSnapshot> {
    let captured_policy = policy.get();
    if !captured_policy.allowed {
        return Err(E_ACCESSDENIED.into());
    }
    let output = Rc::new(RefCell::new(None));
    let result = output.clone();
    let target = context.clone();
    let manager = manager.clone();
    run(
        context,
        client,
        false,
        Box::new(move |cookie| {
            if policy.get() != captured_policy {
                return Err(E_ABORT.into());
            }
            ensure_target(&manager, &target)?;
            let (range, style) = selection(&target, cookie)?;
            // SAFETY: range belongs to the context and cookie of this read session.
            if !same_context(&unsafe { range.GetContext()? }, &target)? {
                return Err(E_ABORT.into());
            }
            let protection = crate::protection::check(&target, &range, cookie)?;
            let original = read_all(&range, cookie)?;
            crate::protection::validate(&target, &range, cookie, protection)?;
            if policy.get() != captured_policy {
                return Err(E_ABORT.into());
            }
            *result.borrow_mut() = Some(SelectionSnapshot {
                context: target,
                range,
                style,
                original,
                policy,
                captured_policy,
                protection,
                owner: thread::current().id(),
            });
            Ok(())
        }),
    )?;
    let snapshot = output.borrow_mut().take().ok_or(E_FAIL)?;
    Ok(snapshot)
}

/// Replace once, entirely inside a fresh write session. Denial is not retried.
/// No input text or range is sent to another thread or process.
pub fn replace_selection(
    manager: &ITfThreadMgr,
    client: u32,
    snapshot: SelectionSnapshot,
    replacement: &str,
) -> Result<()> {
    if snapshot.owner != thread::current().id() {
        return Err(RPC_E_WRONG_THREAD.into());
    }
    let replacement: Vec<u16> = replacement
        .encode_utf16()
        .take(MAX_SELECTION_UNITS + 1)
        .collect();
    if replacement.is_empty() || replacement.len() > MAX_SELECTION_UNITS {
        return Err(E_INVALIDARG.into());
    }
    let context = snapshot.context.clone();
    let manager = manager.clone();
    run(
        &context,
        client,
        true,
        Box::new(move |cookie| {
            let range = validate_snapshot(&manager, &snapshot, cookie)?;
            // SAFETY: validated target and fresh write cookie.
            unsafe { range.SetText(cookie, 0, &replacement)? };
            Ok(())
        }),
    )
}

fn validate_snapshot(
    manager: &ITfThreadMgr,
    snapshot: &SelectionSnapshot,
    cookie: u32,
) -> Result<ITfRange> {
    if snapshot.policy.get() != snapshot.captured_policy || !snapshot.policy.get().allowed {
        return Err(E_ABORT.into());
    }
    ensure_target(manager, &snapshot.context)?;
    let (range, style) = selection(&snapshot.context, cookie)?;
    crate::protection::validate(&snapshot.context, &range, cookie, snapshot.protection)?;
    // SAFETY: endpoint comparisons and reads use this fresh edit cookie.
    unsafe {
        if style != snapshot.style
            || !range
                .IsEqualStart(cookie, &snapshot.range, TF_ANCHOR_START)?
                .as_bool()
            || !range
                .IsEqualEnd(cookie, &snapshot.range, TF_ANCHOR_END)?
                .as_bool()
            || read_all(&range, cookie)? != snapshot.original
        {
            return Err(E_ABORT.into());
        }
    }
    ensure_target(manager, &snapshot.context)?;
    if snapshot.policy.get() != snapshot.captured_policy {
        return Err(E_ABORT.into());
    }
    Ok(range)
}

/// Development-only property round trip, confined to one synchronous write
/// session. This is not a persistent preview or an application rendering test.
#[cfg(feature = "spike")]
pub fn probe_underline(
    manager: &ITfThreadMgr,
    client: u32,
    snapshot: SelectionSnapshot,
) -> Result<()> {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::Variant::{VARIANT, VT_I4};
    if snapshot.owner != thread::current().id() {
        return Err(RPC_E_WRONG_THREAD.into());
    }
    // SAFETY: same-thread COM category manager. RegisterGUID supplies an atom;
    // no provider category or persistent input profile is registered here.
    let categories: ITfCategoryMgr =
        unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
    let atom = unsafe { categories.RegisterGUID(&crate::display::GUID_INPUT_ATTRIBUTE)? };
    let manager = manager.clone();
    let context = snapshot.context.clone();
    run(
        &context,
        client,
        true,
        Box::new(move |cookie| {
            let range = validate_snapshot(&manager, &snapshot, cookie)?;
            // SAFETY: property access stays inside this write session. Refuse an
            // existing property so the probe never overwrites another service.
            unsafe {
                let property = snapshot.context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                // A whole-range GetValue can be empty for mixed values. Inspect
                // each property run as well, before modifying any existing style.
                let mut ranges = None;
                property.EnumRanges(cookie, &mut ranges, &range)?;
                let ranges = ranges.ok_or(E_FAIL)?;
                let mut inspected_runs = 0;
                loop {
                    let mut run = [None];
                    let mut fetched = 0;
                    ranges.Next(&mut run, &mut fetched)?;
                    if fetched == 0 {
                        break;
                    }
                    inspected_runs += 1;
                    if fetched != 1
                        || inspected_runs > MAX_SELECTION_UNITS * 2 + 1
                        || !property
                            .GetValue(cookie, run[0].as_ref().ok_or(E_FAIL)?)?
                            .is_empty()
                    {
                        return Err(E_ABORT.into());
                    }
                }
                // Property enumeration calls external COM code; recheck the
                // grant, focus, selection and text immediately before writing.
                let range = validate_snapshot(&manager, &snapshot, cookie)?;
                property.SetValue(cookie, &range, &VARIANT::from(atom as i32))?;
                let inspected = (|| -> Result<()> {
                    let value = property.GetValue(cookie, &range)?;
                    if value.vt() != VT_I4
                        || value.Anonymous.Anonymous.Anonymous.lVal != atom as i32
                    {
                        return Err(E_FAIL.into());
                    }
                    let prefix = snapshot.context.GetStart(cookie)?;
                    prefix.ShiftEndToRange(cookie, &range, TF_ANCHOR_START)?;
                    let suffix = snapshot.context.GetEnd(cookie)?;
                    suffix.ShiftStartToRange(cookie, &range, TF_ANCHOR_END)?;
                    for outside in [prefix, suffix] {
                        if !outside.IsEmpty(cookie)?.as_bool()
                            && !property.GetValue(cookie, &outside)?.is_empty()
                        {
                            return Err(E_FAIL.into());
                        }
                    }
                    Ok(())
                })();
                // Always attempt removal, including a failed inspection. No text
                // mutation is made, and no attribute survives a successful probe.
                property.Clear(cookie, &range)?;
                inspected?;
                if !property.GetValue(cookie, &range)?.is_empty() {
                    return Err(E_FAIL.into());
                }
            }
            Ok(())
        }),
    )
}
