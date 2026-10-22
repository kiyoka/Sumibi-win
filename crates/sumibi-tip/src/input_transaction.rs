//! A single synchronous write cookie covers supplied insertion or recorded
//! suffix/explicit-selection deletion, caret and composition.
//! This is not rollback: SetText may mutate a store even when it returns failure.
use super::*;
use crate::input_spike::{INPUT_APPLIED, INPUT_APPLIED_UNTRACKED, INPUT_UNCERTAIN};

impl Tracker {
    pub(crate) fn backspace(&mut self, count: u32, outcome: Rc<Cell<u32>>) -> Result<()> {
        if !(1..=MAX_SELECTION_UNITS as u32).contains(&count) {
            return Err(E_INVALIDARG.into());
        }
        self.before(true)?;
        let (before, _) = self.before.take().ok_or(E_ABORT)?;
        let deleted = self
            .session
            .backspace_range(before.point, count as usize)
            .ok_or(E_ABORT)?;
        let previous = self.session.range(before.point).ok_or(E_ABORT)?;
        if self.extent != Some(previous) || !self.owned.can_commit() {
            return Err(E_ABORT.into());
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .filter(|v| *v != u64::MAX)
            .ok_or(E_ABORT)?;
        let after = InputPoint {
            generation: sequence,
            selection: (deleted.0, deleted.0),
            ..before.point
        };
        let mut session = std::mem::take(&mut self.session);
        session.backspaced(before.point, after);
        let range = session.range(after);
        if (range.is_none() && deleted.0 != previous.0)
            || range.is_some_and(|value| value != (previous.0, deleted.0))
        {
            return Err(E_ABORT.into());
        }
        let edit = self.edit();
        let watch = self.watch.clone();
        let boundary = watch.boundary_generation()?;
        let attempted = outcome.clone();
        let finished = Rc::new(Cell::new(None));
        let written = finished.clone();
        run(
            &self.context,
            self.client,
            true,
            Box::new(move |cookie| {
                edit.validate(cookie, before.metadata)?;
                if watch.generation()? != before.notification
                    || Some(extent_of(&edit.owned.range(&edit.context)?)?) != edit.previous
                    || edit.previous != Some(previous)
                    || deleted.0 < previous.0
                    || deleted.1 != previous.1
                {
                    return Err(E_ABORT.into());
                }
                let target = unsafe { edit.context.GetStart(cookie)? };
                unsafe {
                    target
                        .cast::<ITfRangeACP>()?
                        .SetExtent(deleted.0, deleted.1 - deleted.0)?;
                }
                if extent_of(&target)? != deleted
                    || !same_context(&unsafe { target.GetContext()? }, &edit.context)?
                    || metadata_gate(&edit.context, &target, cookie, &edit.owned)?
                        != before.metadata.protection
                {
                    return Err(E_ABORT.into());
                }
                let property = unsafe { edit.context.GetProperty(&GUID_PROP_ATTRIBUTE)? };
                if preview::runs(&property, &target, cookie)?
                    .iter()
                    .any(|(_, value)| !value.is_empty() && !preview::is_atom(value, edit.atom))
                {
                    return Err(E_ABORT.into());
                }
                edit.validate(cookie, before.metadata)?;
                if extent_of(&target)? != deleted
                    || extent_of(&edit.owned.range(&edit.context)?)? != previous
                    || metadata_gate(&edit.context, &target, cookie, &edit.owned)?
                        != before.metadata.protection
                    || watch.generation()? != before.notification
                    || watch.boundary_generation()? != boundary
                {
                    return Err(E_ABORT.into());
                }
                write_body(&edit, cookie, target, &[], deleted.0, &attempted)?;
                let metadata = InputMetadata {
                    selection: after.selection,
                    ..before.metadata
                };
                if let Some(range) = range {
                    edit.update(cookie, range, metadata)?;
                } else {
                    // End only the exact retained object. Full deletion must not
                    // leave an empty composition authorizing later body access.
                    edit.validate(cookie, metadata)?;
                    edit.owned.end(&edit.context, cookie)?;
                    clear_mark(&edit.context, cookie, &edit.marked, edit.atom)?;
                    edit.validate(cookie, metadata)?;
                }
                if watch.boundary_generation()? != boundary {
                    return Err(E_ABORT.into());
                }
                written.set(Some(metadata));
                Ok(())
            }),
        )?;
        PendingInput {
            session,
            sequence,
            range,
            boundary,
            outcome,
        }
        .finish(self, finished.get().ok_or(E_FAIL)?)
    }
    pub(crate) fn insert(&mut self, units: &[u16], outcome: Rc<Cell<u32>>) -> Result<()> {
        if units.is_empty()
            || units.len() > MAX_SELECTION_UNITS
            || char::decode_utf16(units.iter().copied())
                .any(|unit| unit.map_or(true, |value| value.is_control() || value == '\u{fffc}'))
        {
            return Err(E_INVALIDARG.into());
        }
        self.replace(units, outcome)
    }
    pub(crate) fn delete_selection(&mut self, outcome: Rc<Cell<u32>>) -> Result<()> {
        // An explicit Delete retires the old run, never adopts pre-existing
        // text as tracked input. Cleanup must finish before any body write.
        self.cancel(true)?;
        self.replace(&[], outcome)
    }
    fn replace(&mut self, units: &[u16], outcome: Rc<Cell<u32>>) -> Result<()> {
        self.before(false)?;
        let (before, _) = self.before.take().ok_or(E_ABORT)?;
        if units.is_empty()
            && (before.metadata.selection.0 < 0
                || before.metadata.selection.0 >= before.metadata.selection.1
                || before.metadata.selection.1 - before.metadata.selection.0
                    > MAX_SELECTION_UNITS as i32)
        {
            return Err(E_ABORT.into());
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .filter(|v| *v != u64::MAX)
            .ok_or(E_ABORT)?;
        let end = before
            .metadata
            .selection
            .0
            .checked_add(units.len() as i32)
            .ok_or(E_ABORT)?;
        let after = InputPoint {
            generation: sequence,
            selection: (end, end),
            ..before.point
        };
        let mut session = std::mem::take(&mut self.session);
        let range = if units.is_empty() {
            session.clear();
            None
        } else {
            session.inserted(before.point, after, units);
            Some(session.range(after).ok_or(E_ABORT)?)
        };
        let edit = self.edit();
        let watch = self.watch.clone();
        let boundary = watch.boundary_generation()?;
        let text = units.to_vec();
        let attempted = outcome.clone();
        let finished = Rc::new(Cell::new(None));
        let written = finished.clone();
        let result = run(
            &self.context,
            self.client,
            true,
            Box::new(move |cookie| {
                let selected = edit.validate(cookie, before.metadata)?;
                if watch.generation()? != before.notification {
                    return Err(E_ABORT.into());
                }
                if edit.owned.can_commit() {
                    if Some(extent_of(&edit.owned.range(&edit.context)?)?) != edit.previous
                        || edit
                            .previous
                            .is_none_or(|old| old.1 != before.metadata.selection.0)
                        || before.metadata.selection.0 != before.metadata.selection.1
                    {
                        return Err(E_ABORT.into());
                    }
                } else {
                    // Owner rejection/missing sink happens before any body write.
                    edit.owned.start(&edit.context, &selected, cookie)?;
                }
                // Starting our composition may emit metadata notifications.
                // Validate target/protection again after that external COM call.
                edit.validate(cookie, before.metadata)?;
                let property = unsafe { edit.context.GetProperty(&GUID_PROP_ATTRIBUTE)? };
                if preview::runs(&property, &selected, cookie)?
                    .iter()
                    .any(|(_, value)| !value.is_empty() && !preview::is_atom(value, edit.atom))
                {
                    return Err(E_ABORT.into());
                }
                let baseline = watch.generation()?;
                edit.validate(cookie, before.metadata)?;
                if watch.generation()? != baseline
                    || watch.boundary_generation()? != boundary
                    || !same_context(&unsafe { selected.GetContext()? }, &edit.context)?
                    || extent_of(&selected)? != before.metadata.selection
                {
                    return Err(E_ABORT.into());
                }
                write_body(
                    &edit,
                    cookie,
                    selected,
                    &text,
                    before.metadata.selection.0,
                    &attempted,
                )?;
                let metadata = InputMetadata {
                    selection: (end, end),
                    ..before.metadata
                };
                if let Some(range) = range {
                    edit.update(cookie, range, metadata)?;
                } else {
                    edit.validate(cookie, metadata)?;
                    edit.owned.end(&edit.context, cookie)?;
                    clear_mark(&edit.context, cookie, &edit.marked, edit.atom)?;
                    edit.validate(cookie, metadata)?;
                }
                if watch.boundary_generation()? != boundary {
                    return Err(E_ABORT.into());
                }
                written.set(Some(metadata));
                Ok(())
            }),
        );
        result?;
        PendingInput {
            session,
            sequence,
            range,
            boundary,
            outcome,
        }
        .finish(self, finished.get().ok_or(E_FAIL)?)
    }
}

struct PendingInput {
    session: InputSession,
    sequence: u64,
    range: Option<(i32, i32)>,
    boundary: u64,
    outcome: Rc<Cell<u32>>,
}
impl PendingInput {
    fn finish(self, tracker: &mut Tracker, expected: InputMetadata) -> Result<()> {
        // OnEndEdit is delivered after the cookie. Protection/focus history is
        // independent of our own text/attribute notification and cannot reset.
        let notification = tracker.watch.generation()?;
        let metadata = tracker.inspect(false)?;
        if tracker.watch.generation()? != notification
            || tracker.watch.boundary_generation()? != self.boundary
            || metadata.selection != expected.selection
            || !metadata.same_protection(&expected)
        {
            return Err(E_ABORT.into());
        }
        tracker.session = self.session;
        tracker.sequence = self.sequence;
        tracker.metadata = Some(metadata);
        tracker.settled = Some(notification);
        tracker.extent = self.range;
        if self.range.is_none() {
            tracker.owned = OwnedComposition::new();
        }
        tracker.auto_selected = false;
        self.outcome.set(INPUT_APPLIED);
        Ok(())
    }
}

/// The only body write for these operations. All authorization precedes this
/// helper; errors after the call can never grant host replay or rollback.
fn write_body(
    edit: &CompositionEdit,
    cookie: u32,
    selected: ITfRange,
    text: &[u16],
    start: i32,
    attempted: &Cell<u32>,
) -> Result<()> {
    let end = start.checked_add(text.len() as i32).ok_or(E_ABORT)?;
    attempted.set(INPUT_UNCERTAIN);
    let status = unsafe {
        (selected.vtable().SetText)(
            selected.as_raw(),
            cookie,
            0,
            windows::core::PCWSTR(text.as_ptr()),
            text.len() as i32,
        )
    };
    if status != S_OK {
        return Err(if status.is_err() { status } else { E_FAIL }.into());
    }
    attempted.set(INPUT_APPLIED_UNTRACKED);
    if extent_of(&selected)? != (start, end) {
        return Err(E_ABORT.into());
    }
    unsafe {
        selected.Collapse(cookie, TF_ANCHOR_END)?;
    }
    let mut caret = TF_SELECTION {
        range: ManuallyDrop::new(Some(selected)),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_END,
            fInterimChar: false.into(),
        },
    };
    let set = unsafe {
        edit.context
            .SetSelection(cookie, std::slice::from_ref(&caret))
    };
    unsafe {
        ManuallyDrop::drop(&mut caret.range);
    }
    set
}
