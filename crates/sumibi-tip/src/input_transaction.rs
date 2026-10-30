//! Synchronous transactions for supplied insertion, recorded/selected deletion
//! and caret movement, with the same cookie for their composition metadata.
//! This is not rollback: SetText may mutate a store even when it returns failure.
use super::*;
use crate::input_spike::{INPUT_APPLIED, INPUT_APPLIED_UNTRACKED, INPUT_UNCERTAIN};

impl Tracker {
    pub(crate) fn backspace(&mut self, count: u32, outcome: Rc<Cell<u32>>) -> Result<()> {
        self.erase(count, false, outcome)
    }
    pub(crate) fn forward_delete(&mut self, count: u32, outcome: Rc<Cell<u32>>) -> Result<()> {
        self.erase(count, true, outcome)
    }
    fn erase(&mut self, count: u32, forward: bool, outcome: Rc<Cell<u32>>) -> Result<()> {
        if !(1..=MAX_SELECTION_UNITS as u32).contains(&count) {
            return Err(E_INVALIDARG.into());
        }
        self.before(true)?;
        let (before, _) = self.before.take().ok_or(E_ABORT)?;
        let deleted = if forward {
            self.session
                .forward_delete_range(before.point, count as usize)
        } else {
            self.session.backspace_range(before.point, count as usize)
        }
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
        if forward {
            session.deleted_forward(before.point, after, count as usize);
        } else {
            session.deleted_backward(before.point, after, count as usize);
        }
        let range = session.range(after);
        let end = previous
            .1
            .checked_sub(deleted.1 - deleted.0)
            .ok_or(E_ABORT)?;
        if (range.is_none() && end != previous.0)
            || range.is_some_and(|value| value != (previous.0, end))
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
                    || deleted.1 > previous.1
                    || deleted.0 >= deleted.1
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
                    active_start: false,
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
            document_end: None,
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
        // Retain only an exact live recorded selection. External/untracked
        // selections keep the original retire-before-write behavior.
        if self.observe().ok().is_some_and(|before| {
            self.owned.can_commit()
                && self.session.range(before.point) == self.extent
                && self.session.replacement_range(before.point, 0).is_some()
        }) {
            return self.replace(&[], outcome);
        }
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
        let previous = if self.owned.can_commit() {
            let recorded = self.session.range(before.point).ok_or(E_ABORT)?;
            if self.extent != Some(recorded) {
                return Err(E_ABORT.into());
            }
            Some(recorded)
        } else {
            None
        };
        let mut session = std::mem::take(&mut self.session);
        let range = if let Some(old) = previous {
            let expected_end = old
                .1
                .checked_sub(before.point.selection.1 - before.point.selection.0)
                .and_then(|end| end.checked_add(units.len() as i32))
                .ok_or(E_ABORT)?;
            if before.point.selection.0 == before.point.selection.1 {
                session.inserted_at_caret(before.point, after, units);
            } else {
                if session.replacement_range(before.point, units.len())
                    != Some((old.0, expected_end))
                {
                    return Err(E_ABORT.into());
                }
                session.replaced_selection(before.point, after, units);
            }
            let recorded = session.range(after);
            if recorded != (expected_end > old.0).then_some((old.0, expected_end)) {
                return Err(E_ABORT.into());
            }
            recorded
        } else if units.is_empty() {
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
                let checked = if let Some(old) = previous {
                    let owned = edit.owned.range(&edit.context)?;
                    if extent_of(&owned)? != old
                        || edit.previous != Some(old)
                        || before.metadata.selection.0 < old.0
                        || before.metadata.selection.1 > old.1
                        || metadata_gate(&edit.context, &owned, cookie, &edit.owned)?
                            != before.metadata.protection
                    {
                        return Err(E_ABORT.into());
                    }
                    owned
                } else {
                    // Owner rejection/missing sink happens before any body write.
                    edit.owned.start(&edit.context, &selected, cookie)?;
                    selected.clone()
                };
                // Starting our composition may emit metadata notifications.
                // Validate target/protection again after that external COM call.
                edit.validate(cookie, before.metadata)?;
                let property = unsafe { edit.context.GetProperty(&GUID_PROP_ATTRIBUTE)? };
                if preview::runs(&property, &checked, cookie)?
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
                    || previous.is_some_and(|old| extent_of(&checked).ok() != Some(old))
                    || previous.is_some_and(|old| {
                        edit.owned
                            .range(&edit.context)
                            .and_then(|owned| extent_of(&owned))
                            .ok()
                            != Some(old)
                    })
                    || metadata_gate(&edit.context, &checked, cookie, &edit.owned)?
                        != before.metadata.protection
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
                    active_start: false,
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
            document_end: None,
            boundary,
            outcome,
        }
        .finish(self, finished.get().ok_or(E_FAIL)?)
    }
}

impl Tracker {
    pub(crate) fn move_caret(
        &mut self,
        backward: bool,
        count: u32,
        outcome: Rc<Cell<u32>>,
    ) -> Result<()> {
        self.move_selection(backward, false, count, outcome)
    }
    pub(crate) fn move_selection(
        &mut self,
        backward: bool,
        extend: bool,
        count: u32,
        outcome: Rc<Cell<u32>>,
    ) -> Result<()> {
        self.navigate_selection(backward, extend, count, false, outcome)
    }
    pub(crate) fn move_document_edge(
        &mut self,
        to_end: bool,
        extend: bool,
        count: u32,
        outcome: Rc<Cell<u32>>,
    ) -> Result<()> {
        self.navigate_selection(!to_end, extend, count, true, outcome)
    }
    fn document_end(&self, expected: InputMetadata) -> Result<i32> {
        let edit = self.edit();
        let output = Rc::new(Cell::new(None));
        let result = output.clone();
        run(
            &self.context,
            self.client,
            false,
            Box::new(move |cookie| {
                edit.validate(cookie, expected)?;
                let end = read_document_end(&edit.context, cookie)?;
                edit.validate(cookie, expected)?;
                result.set(Some(end));
                Ok(())
            }),
        )?;
        output.get().ok_or_else(|| E_FAIL.into())
    }
    fn navigate_selection(
        &mut self,
        backward: bool,
        extend: bool,
        count: u32,
        document_edge: bool,
        outcome: Rc<Cell<u32>>,
    ) -> Result<()> {
        if !(1..=MAX_SELECTION_UNITS as u32).contains(&count) {
            return Err(E_INVALIDARG.into());
        }
        let direction = if backward {
            sumibi_core::grapheme::Direction::Backward
        } else {
            sumibi_core::grapheme::Direction::Forward
        };
        let before = self.observe()?;
        if self.ready.is_some() || self.before.is_some() || self.cleanup_pending {
            return Err(E_ABORT.into());
        }
        let previous = self.session.range(before.point).ok_or(E_ABORT)?;
        if self.extent != Some(previous) || !self.owned.can_commit() {
            return Err(E_ABORT.into());
        }
        let document_end = if document_edge {
            Some(self.document_end(before.metadata)?)
        } else {
            None
        };
        if self.watch.generation()? != before.notification {
            return Err(E_ABORT.into());
        }
        let edge = document_end.map(|document_end| sumibi_core::input::EdgeMove {
            to_end: !backward,
            extend,
            active_start: before.metadata.active_start,
            document_end,
            count: count as usize,
        });
        let (selected_range, active_start) = if let Some(edge) = edge {
            self.session.edge_range(before.point, edge)
        } else {
            self.session.selection_range(
                before.point,
                direction,
                extend,
                before.metadata.active_start,
                count as usize,
            )
        }
        .ok_or(E_ABORT)?;
        let sequence = self
            .sequence
            .checked_add(1)
            .filter(|v| *v != u64::MAX)
            .ok_or(E_ABORT)?;
        let after = InputPoint {
            generation: sequence,
            selection: selected_range,
            ..before.point
        };
        let mut session = std::mem::take(&mut self.session);
        if let Some(edge) = edge {
            session.moved_to_edge(before.point, after, edge);
        } else {
            session.selected(
                before.point,
                after,
                direction,
                extend,
                before.metadata.active_start,
                count as usize,
            );
        }
        if session.range(after) != Some(previous) {
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
                check_document_end(&edit.context, cookie, document_end)?;
                let owned = edit.owned.range(&edit.context)?;
                if extent_of(&owned)? != previous
                    || edit.previous != Some(previous)
                    || metadata_gate(&edit.context, &owned, cookie, &edit.owned)?
                        != before.metadata.protection
                    || watch.generation()? != before.notification
                {
                    return Err(E_ABORT.into());
                }
                let target = unsafe { edit.context.GetStart(cookie)? };
                unsafe {
                    target
                        .cast::<ITfRangeACP>()?
                        .SetExtent(selected_range.0, selected_range.1 - selected_range.0)?;
                }
                if extent_of(&target)? != selected_range
                    || !same_context(&unsafe { target.GetContext()? }, &edit.context)?
                    || metadata_gate(&edit.context, &target, cookie, &edit.owned)?
                        != before.metadata.protection
                {
                    return Err(E_ABORT.into());
                }
                edit.validate(cookie, before.metadata)?;
                check_document_end(&edit.context, cookie, document_end)?;
                if watch.generation()? != before.notification
                    || watch.boundary_generation()? != boundary
                    || extent_of(&edit.owned.range(&edit.context)?)? != previous
                {
                    return Err(E_ABORT.into());
                }
                // A failed SetSelection can still have moved the caret. Never apply
                // the original host movement a second time after this point.
                attempted.set(INPUT_UNCERTAIN);
                place_selection(&edit, cookie, target, active_start)?;
                attempted.set(INPUT_APPLIED_UNTRACKED);
                let metadata = InputMetadata {
                    selection: after.selection,
                    active_start,
                    ..before.metadata
                };
                edit.validate(cookie, metadata)?;
                check_document_end(&edit.context, cookie, document_end)?;
                if extent_of(&edit.owned.range(&edit.context)?)? != previous
                    || metadata_gate(&edit.context, &owned, cookie, &edit.owned)?
                        != before.metadata.protection
                    || watch.boundary_generation()? != boundary
                {
                    return Err(E_ABORT.into());
                }
                written.set(Some(metadata));
                Ok(())
            }),
        )?;
        PendingInput {
            session,
            sequence,
            range: Some(previous),
            document_end,
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
    document_end: Option<i32>,
    boundary: u64,
    outcome: Rc<Cell<u32>>,
}
impl PendingInput {
    fn finish(self, tracker: &mut Tracker, expected: InputMetadata) -> Result<()> {
        // OnEndEdit is delivered after the cookie. Protection/focus history is
        // independent of our own text/attribute notification and cannot reset.
        let notification = tracker.watch.generation()?;
        let metadata = tracker.inspect(false)?;
        if let Some(expected) = self.document_end {
            if tracker.document_end(metadata)? != expected {
                return Err(E_ABORT.into());
            }
        }
        if tracker.watch.generation()? != notification
            || tracker.watch.boundary_generation()? != self.boundary
            || !metadata.same_selection(&expected)
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

/// Coordinate metadata only. Never GetText or enumerate the full document's
/// attributes; ordinary host Home/End semantics are not inferred from this.
fn read_document_end(context: &ITfContext, cookie: u32) -> Result<i32> {
    let first = unsafe { context.GetStart(cookie)? };
    let last = unsafe { context.GetEnd(cookie)? };
    let (end, end_again) = extent_of(&last)?;
    if extent_of(&first)? != (0, 0)
        || end != end_again
        || !same_context(&unsafe { first.GetContext()? }, context)?
        || !same_context(&unsafe { last.GetContext()? }, context)?
    {
        return Err(E_ABORT.into());
    }
    Ok(end)
}
fn check_document_end(context: &ITfContext, cookie: u32, expected: Option<i32>) -> Result<()> {
    if let Some(expected) = expected {
        if read_document_end(context, cookie)? != expected {
            return Err(E_ABORT.into());
        }
    }
    Ok(())
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
    place_caret(edit, cookie, selected)
}
fn place_caret(edit: &CompositionEdit, cookie: u32, selected: ITfRange) -> Result<()> {
    unsafe {
        selected.Collapse(cookie, TF_ANCHOR_END)?;
    }
    place_selection(edit, cookie, selected, false)
}
fn place_selection(
    edit: &CompositionEdit,
    cookie: u32,
    selected: ITfRange,
    active_start: bool,
) -> Result<()> {
    let mut caret = TF_SELECTION {
        range: ManuallyDrop::new(Some(selected)),
        style: TF_SELECTIONSTYLE {
            ase: if active_start { TF_AE_START } else { TF_AE_END },
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
