//! A single synchronous write cookie covers insertion, caret and composition.
//! This is not rollback: SetText may mutate a store even when it returns failure.
use super::*;
use crate::input_spike::{INPUT_APPLIED, INPUT_APPLIED_UNTRACKED, INPUT_UNCERTAIN};

impl Tracker {
    pub(crate) fn insert(&mut self, units: &[u16], outcome: Rc<Cell<u32>>) -> Result<()> {
        if units.is_empty()
            || units.len() > MAX_SELECTION_UNITS
            || char::decode_utf16(units.iter().copied())
                .any(|unit| unit.map_or(true, |value| value.is_control() || value == '\u{fffc}'))
        {
            return Err(E_INVALIDARG.into());
        }
        self.before(false)?;
        let (before, _) = self.before.take().ok_or(E_ABORT)?;
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
        session.inserted(before.point, after, units);
        let range = session.range(after).ok_or(E_ABORT)?;
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
                // Starting our empty composition may emit metadata notifications.
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
                // Commit the no-replay disposition BEFORE crossing SetText. Raw
                // HRESULT matters: S_FALSE is not a confirmed successful insertion.
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
                // Do not undo or retry the body if any following metadata operation
                // fails. End only our composition in the outer cleanup path.
                if extent_of(&selected)? != (before.metadata.selection.0, end) {
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
                set?;
                let metadata = InputMetadata {
                    selection: (end, end),
                    ..before.metadata
                };
                edit.update(cookie, range, metadata)?;
                if watch.boundary_generation()? != boundary {
                    return Err(E_ABORT.into());
                }
                written.set(Some(metadata));
                Ok(())
            }),
        );
        result?;
        let expected = finished.get().ok_or(E_FAIL)?;
        // OnEndEdit is delivered after the cookie. Protection/focus history is
        // independent of our own text/attribute notification and cannot reset.
        let notification = self.watch.generation()?;
        let metadata = self.inspect(false)?;
        if self.watch.generation()? != notification
            || self.watch.boundary_generation()? != boundary
            || metadata.selection != expected.selection
            || !metadata.same_protection(&expected)
        {
            return Err(E_ABORT.into());
        }
        self.session = session;
        self.sequence = sequence;
        self.metadata = Some(metadata);
        self.settled = Some(notification);
        self.extent = Some(range);
        self.auto_selected = false;
        outcome.set(INPUT_APPLIED);
        Ok(())
    }
}
