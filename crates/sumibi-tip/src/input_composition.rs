//! Cooperating-host input composition. Before/After updates metadata after one
//! host edit; the independent writer inserts supplied text in one write cookie.
use super::*;
use crate::owned_composition::OwnedComposition;
use sumibi_core::input::{InputPoint, InputSession, TrackedInput};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::VARIANT;

#[path = "input_transaction.rs"]
mod transaction;

struct Observation {
    point: InputPoint,
    metadata: InputMetadata,
    notification: u64,
}
struct Ready {
    run: Option<TrackedInput>,
    metadata: InputMetadata,
    notification: u64,
}
struct CleanupRetry {
    notification: Option<u64>,
}
pub(crate) struct Tracker {
    manager: ITfThreadMgr,
    pub(crate) context: ITfContext,
    client: u32,
    watch: manual::InputWatch,
    sequence: u64,
    session: InputSession,
    before: Option<(Observation, bool)>,
    settled: Option<u64>,
    metadata: Option<InputMetadata>,
    ready: Option<Ready>,
    auto_selected: bool,
    owned: Rc<OwnedComposition>,
    marked: Rc<RefCell<Option<ITfRange>>>,
    extent: Option<(i32, i32)>,
    atom: i32,
    cleanup_pending: bool,
    cleanup_retry: Option<CleanupRetry>,
}
impl Tracker {
    pub(crate) fn set_wake(&self, wake: Option<Rc<dyn Fn()>>) -> Result<()> {
        self.watch.set_wake(wake)
    }
    /// Called outside notifications by the owner loop. Metadata-only cleanup
    /// is retried only for a new notification, never by spinning or replaying text.
    pub(crate) fn poll(&mut self, retired: bool) -> Result<()> {
        if self.cleanup_pending {
            if self
                .cleanup_retry
                .as_ref()
                .is_some_and(|old| old.notification == self.watch.generation().ok())
            {
                return Ok(());
            }
            return self.cancel(false);
        }
        if retired {
            return self.cancel(false);
        }
        if let Some(ready) = &self.ready {
            if self.watch.generation().ok() != Some(ready.notification) {
                return self.cancel(false);
            }
        }
        if self.extent.is_some() && self.observe().is_err() {
            return self.cancel(false);
        }
        Ok(())
    }
    pub(crate) fn new(manager: &ITfThreadMgr, context: &ITfContext, client: u32) -> Result<Self> {
        let categories: ITfCategoryMgr =
            unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
        Ok(Self {
            manager: manager.clone(),
            context: context.clone(),
            client,
            watch: manual::InputWatch::attach(manager, context)?,
            sequence: 1,
            session: InputSession::default(),
            before: None,
            settled: None,
            metadata: None,
            ready: None,
            auto_selected: false,
            owned: OwnedComposition::new(),
            marked: Rc::new(RefCell::new(None)),
            extent: None,
            atom: unsafe { categories.RegisterGUID(&crate::display::GUID_INPUT_ATTRIBUTE)? } as i32,
            cleanup_pending: false,
            cleanup_retry: None,
        })
    }
    fn inspect(&self, exact: bool) -> Result<InputMetadata> {
        let manager = self.manager.clone();
        let context = self.context.clone();
        let owned = self.owned.clone();
        let extent = self.extent;
        let output = Rc::new(RefCell::new(None));
        let result = output.clone();
        run(
            &self.context,
            self.client,
            false,
            Box::new(move |cookie| {
                ensure_target(&manager, &context)?;
                let (selected, style) = selection(&context, cookie)?;
                if style.ase != TF_AE_START && style.ase != TF_AE_END {
                    return Err(E_ABORT.into());
                }
                if !same_context(&unsafe { selected.GetContext()? }, &context)? {
                    return Err(E_ABORT.into());
                }
                let protection = metadata_gate(&context, &selected, cookie, &owned)?;
                let selection = extent_of(&selected)?;
                if owned.can_commit()
                    && exact
                    && Some(extent_of(&owned.range(&context)?)?) != extent
                {
                    return Err(E_ABORT.into());
                }
                if metadata_gate(&context, &selected, cookie, &owned)? != protection {
                    return Err(E_ABORT.into());
                }
                ensure_target(&manager, &context)?;
                *result.borrow_mut() = Some(InputMetadata {
                    selection,
                    active_start: style.ase == TF_AE_START,
                    protection,
                });
                Ok(())
            }),
        )?;
        let metadata = output.borrow_mut().take().ok_or(E_FAIL)?;
        Ok(metadata)
    }
    fn point(&self, metadata: InputMetadata) -> InputPoint {
        // Advance the Core epoch only after a validated input/caret transaction
        // or observed host edit. Check raw TSF notifications against settled;
        // its own composition/underline notifications never bless old text.
        InputPoint {
            context: 1,
            generation: self.sequence,
            composition_generation: 0,
            selection: metadata.selection,
            allowed: true,
        }
    }
    fn observe(&self) -> Result<Observation> {
        let notification = self.watch.generation()?;
        let metadata = self.inspect(true)?;
        if self.watch.generation()? != notification
            || self.settled.is_some_and(|old| old != notification)
            || self.metadata.is_some_and(|old| {
                !old.same_protection(&metadata) || old.active_start != metadata.active_start
            })
        {
            return Err(E_ABORT.into());
        }
        Ok(Observation {
            point: self.point(metadata),
            metadata,
            notification,
        })
    }
    pub(crate) fn before(&mut self, backspace: bool) -> Result<()> {
        if self.cleanup_pending {
            self.cancel(false)?;
        }
        self.ready = None;
        self.before = None;
        let observed = match self.observe() {
            Ok(observed) => observed,
            Err(_) => {
                self.cancel(false)?;
                self.observe()?
            }
        };
        self.before = Some((observed, backspace));
        Ok(())
    }
    pub(crate) fn after(&mut self, units: &[u16]) -> Result<()> {
        let result = self.after_inner(units);
        if result.is_err() {
            let _ = self.cancel(false);
        }
        result
    }
    fn after_inner(&mut self, units: &[u16]) -> Result<()> {
        let (before, backspace) = self.before.take().ok_or(E_ABORT)?;
        let metadata = self.inspect(false)?;
        if self.watch.generation()? <= before.notification
            || !before.metadata.same_protection(&metadata)
        {
            return Err(E_ABORT.into());
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .filter(|v| *v != u64::MAX)
            .ok_or(E_ABORT)?;
        let after = self.point(metadata);
        if backspace {
            self.session.backspaced(before.point, after);
        } else {
            self.session.inserted(before.point, after, units);
        }
        self.auto_selected = false;
        let Some(range) = self.session.range(after) else {
            return self.cancel(false);
        };
        self.update(range, metadata)?;
        let notification = self.watch.generation()?;
        let latest = self.inspect(true)?;
        if self.watch.generation()? != notification
            || !latest.same_selection(&metadata)
            || !metadata.same_protection(&latest)
        {
            return Err(E_ABORT.into());
        }
        self.settled = Some(notification);
        self.metadata = Some(latest);
        Ok(())
    }
    fn update(&mut self, range: (i32, i32), expected: InputMetadata) -> Result<()> {
        let edit = self.edit();
        run(
            &self.context,
            self.client,
            true,
            Box::new(move |cookie| edit.update(cookie, range, expected)),
        )?;
        self.extent = Some(range);
        Ok(())
    }
    fn edit(&self) -> CompositionEdit {
        CompositionEdit {
            context: self.context.clone(),
            manager: self.manager.clone(),
            owned: self.owned.clone(),
            marked: self.marked.clone(),
            atom: self.atom,
            previous: self.extent,
        }
    }
    pub(crate) fn cancel(&mut self, user_boundary: bool) -> Result<()> {
        self.cleanup_pending = true;
        self.cleanup_retry = None;
        self.session.clear();
        self.before = None;
        self.settled = None;
        self.metadata = None;
        self.ready = None;
        if user_boundary {
            self.auto_selected = false;
        }
        let context = self.context.clone();
        let owned = self.owned.clone();
        let marked = self.marked.clone();
        let atom = self.atom;
        if owned.can_commit() || marked.borrow().is_some() {
            let cleanup = run(
                &self.context,
                self.client,
                true,
                Box::new(move |cookie| {
                    owned.end(&context, cookie)?;
                    clear_mark(&context, cookie, &marked, atom)
                }),
            );
            if let Err(error) = cleanup {
                // Failed metadata writes may themselves emit OnEndEdit. Their
                // wake cannot trigger the same failed write in a tight loop.
                self.cleanup_retry = Some(CleanupRetry {
                    notification: self.watch.generation().ok(),
                });
                return Err(error);
            }
        }
        self.owned = OwnedComposition::new();
        self.extent = None;
        self.cleanup_pending = false;
        self.cleanup_retry = None;
        Ok(())
    }
    pub(crate) fn prepare(&mut self) -> Result<()> {
        self.ready = None;
        self.before = None;
        if self.auto_selected || self.cleanup_pending {
            return Err(E_ABORT.into());
        }
        let boundary = self.watch.boundary_generation()?;
        let observed = self.observe()?;
        let collapsed = observed.metadata.selection.0 == observed.metadata.selection.1;
        let tracked = if collapsed {
            Some(self.session.take(observed.point).ok_or(E_ABORT)?)
        } else {
            None
        };
        let range = tracked
            .as_ref()
            .map_or(observed.metadata.selection, TrackedInput::range);
        if range.0 >= range.1 || range.1 - range.0 > MAX_SELECTION_UNITS as i32 {
            return Err(E_ABORT.into());
        }
        self.session.clear();
        self.auto_selected = collapsed;
        let context = self.context.clone();
        let manager = self.manager.clone();
        let owned = self.owned.clone();
        let marked = self.marked.clone();
        let atom = self.atom;
        let expected = observed.metadata;
        run(
            &self.context,
            self.client,
            true,
            Box::new(move |cookie| {
                ensure_target(&manager, &context)?;
                owned.end(&context, cookie)?;
                if owned.was_terminated() {
                    return Err(E_ABORT.into());
                }
                clear_mark(&context, cookie, &marked, atom)?;
                let target = unsafe { context.GetStart(cookie)? };
                unsafe {
                    target
                        .cast::<ITfRangeACP>()?
                        .SetExtent(range.0, range.1 - range.0)?;
                }
                if crate::protection::check(&context, &target, cookie)? != expected.protection {
                    return Err(E_ABORT.into());
                }
                ensure_target(&manager, &context)?;
                // Only a recorded run needs an automatic selection. Keep the
                // endpoints and direction of an explicit selection unchanged.
                if !collapsed {
                    return Ok(());
                }
                let mut selected = TF_SELECTION {
                    range: ManuallyDrop::new(Some(target)),
                    style: TF_SELECTIONSTYLE {
                        ase: TF_AE_END,
                        fInterimChar: false.into(),
                    },
                };
                let result =
                    unsafe { context.SetSelection(cookie, std::slice::from_ref(&selected)) };
                unsafe {
                    ManuallyDrop::drop(&mut selected.range);
                }
                result?;
                Ok(())
            }),
        )?;
        self.extent = None;
        self.owned = OwnedComposition::new();
        let notification = self.watch.generation()?;
        let metadata = self.inspect(false)?;
        if self.watch.generation()? != notification
            || self.watch.boundary_generation()? != boundary
            || metadata.selection != range
            || metadata.active_start != (!collapsed && expected.active_start)
            || !metadata.same_protection(&expected)
        {
            return Err(E_ABORT.into());
        }
        self.ready = Some(Ready {
            run: tracked,
            metadata,
            notification,
        });
        self.settled = None;
        self.metadata = None;
        Ok(())
    }
    pub(crate) fn source(&mut self, units: &[u16]) -> Result<()> {
        let ready = self.ready.take().ok_or(E_ABORT)?;
        if ready.run.as_ref().is_some_and(|run| !run.matches(units))
            || self.watch.generation()? != ready.notification
        {
            return Err(E_ABORT.into());
        }
        let metadata = self.inspect(false)?;
        if !metadata.same_selection(&ready.metadata)
            || !metadata.same_protection(&ready.metadata)
            || self.watch.generation()? != ready.notification
        {
            return Err(E_ABORT.into());
        }
        Ok(())
    }
}
impl Drop for Tracker {
    fn drop(&mut self) {
        let _ = boundary(|| self.cancel(false));
    }
}
fn extent_of(range: &ITfRange) -> Result<(i32, i32)> {
    let mut start = 0i32;
    let mut length = 0i32;
    unsafe {
        range
            .cast::<ITfRangeACP>()?
            .GetExtent(&mut start, &mut length)?;
    }
    if start < 0 || length < 0 {
        return Err(E_ABORT.into());
    }
    Ok((start, start.checked_add(length).ok_or(E_ABORT)?))
}
fn metadata_gate(
    context: &ITfContext,
    range: &ITfRange,
    cookie: u32,
    owned: &OwnedComposition,
) -> Result<crate::protection::Stamp> {
    if owned.can_commit() {
        crate::protection::owned_metadata(context, range, cookie, owned)
    } else {
        crate::protection::check(context, range, cookie)
    }
}
fn clear_mark(
    context: &ITfContext,
    cookie: u32,
    marked: &RefCell<Option<ITfRange>>,
    atom: i32,
) -> Result<()> {
    let target = marked.borrow().clone();
    if let Some(target) = target {
        let property = unsafe { context.GetProperty(&GUID_PROP_ATTRIBUTE)? };
        for (range, value) in preview::runs(&property, &target, cookie)? {
            if preview::is_atom(&value, atom) {
                unsafe {
                    property.Clear(cookie, &range)?;
                }
            }
        }
        marked.borrow_mut().take();
    }
    Ok(())
}

/// Shared cookie-local metadata operations for host-after-edit and DLL insertion.
struct CompositionEdit {
    context: ITfContext,
    manager: ITfThreadMgr,
    owned: Rc<OwnedComposition>,
    marked: Rc<RefCell<Option<ITfRange>>>,
    atom: i32,
    previous: Option<(i32, i32)>,
}
impl CompositionEdit {
    fn validate(&self, cookie: u32, expected: InputMetadata) -> Result<ITfRange> {
        ensure_target(&self.manager, &self.context)?;
        let (selected, style) = selection(&self.context, cookie)?;
        if style.ase != TF_AE_START && style.ase != TF_AE_END {
            return Err(E_ABORT.into());
        }
        if !same_context(&unsafe { selected.GetContext()? }, &self.context)?
            || extent_of(&selected)? != expected.selection
            || (style.ase == TF_AE_START) != expected.active_start
            || metadata_gate(&self.context, &selected, cookie, &self.owned)? != expected.protection
        {
            return Err(E_ABORT.into());
        }
        Ok(selected)
    }
    fn update(&self, cookie: u32, range: (i32, i32), expected: InputMetadata) -> Result<()> {
        if range.0 < 0 || range.1 <= range.0 || range.1 - range.0 > MAX_SELECTION_UNITS as i32 {
            return Err(E_ABORT.into());
        }
        self.validate(cookie, expected)?;
        if self.previous.is_some_and(|old| old.0 != range.0) {
            return Err(E_ABORT.into());
        }
        let target = unsafe { self.context.GetStart(cookie)? };
        unsafe {
            target
                .cast::<ITfRangeACP>()?
                .SetExtent(range.0, range.1 - range.0)?;
        }
        if extent_of(&target)? != range
            || !same_context(&unsafe { target.GetContext()? }, &self.context)?
        {
            return Err(E_ABORT.into());
        }
        if metadata_gate(&self.context, &target, cookie, &self.owned)? != expected.protection {
            return Err(E_ABORT.into());
        }
        unsafe {
            let property = self.context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
            if preview::runs(&property, &target, cookie)?
                .iter()
                .any(|(_, value)| !value.is_empty() && !preview::is_atom(value, self.atom))
            {
                return Err(E_ABORT.into());
            }
            clear_mark(&self.context, cookie, &self.marked, self.atom)?;
            *self.marked.borrow_mut() = Some(target.Clone()?);
            property.SetValue(cookie, &target, &VARIANT::from(self.atom))?;
            if self.owned.can_commit() {
                self.owned.shift_end(&self.context, cookie, &target)?;
            } else {
                self.owned.start(&self.context, &target, cookie)?;
            }
        }
        if extent_of(&self.owned.range(&self.context)?)? != range
            || metadata_gate(&self.context, &target, cookie, &self.owned)? != expected.protection
        {
            return Err(E_ABORT.into());
        }
        ensure_target(&self.manager, &self.context)
    }
}
