//! Development-only selection composition and underline between edit sessions.
use super::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::{VARIANT, VT_I4};

/// Retain until cancel succeeds, or the context is destroyed. Drop only makes
/// a best-effort cleanup: a host can deny synchronous write access.
pub struct Preview {
    manager: ITfThreadMgr,
    client: u32,
    snapshot: Rc<SelectionSnapshot>,
    atom: i32,
    active: Rc<Cell<bool>>,
    composition: Rc<crate::owned_composition::OwnedComposition>,
}

fn runs(
    property: &ITfProperty,
    target: &ITfRange,
    cookie: u32,
) -> Result<Vec<(ITfRange, VARIANT)>> {
    // SAFETY: caller is inside the matching edit session; all ranges stay local.
    unsafe {
        let mut enumerator = None;
        property.EnumRanges(cookie, &mut enumerator, target)?;
        let enumerator = enumerator.ok_or(E_FAIL)?;
        let mut output = Vec::new();
        let mut inspected = 0;
        loop {
            let mut slot = [None];
            let mut count = 0;
            enumerator.Next(&mut slot, &mut count)?;
            if count == 0 {
                return Ok(output);
            }
            inspected += 1;
            if count != 1 || inspected > MAX_SELECTION_UNITS * 2 + 1 {
                return Err(E_ABORT.into());
            }
            let range = slot[0].take().ok_or(E_FAIL)?;
            // Do not rely on an enumerator clipping a property run to the
            // requested target; cleanup must never reach outside our mark.
            if range.CompareStart(cookie, target, TF_ANCHOR_START)? < 0 {
                range.ShiftStartToRange(cookie, target, TF_ANCHOR_START)?;
            }
            if range.CompareEnd(cookie, target, TF_ANCHOR_END)? > 0 {
                range.ShiftEndToRange(cookie, target, TF_ANCHOR_END)?;
            }
            if range.IsEmpty(cookie)?.as_bool() {
                continue;
            }
            let value = property.GetValue(cookie, &range)?;
            output.push((range, value));
        }
    }
}

fn is_atom(value: &VARIANT, atom: i32) -> bool {
    // SAFETY: the union field is read only after checking its discriminator.
    value.vt() == VT_I4 && unsafe { value.Anonymous.Anonymous.Anonymous.lVal == atom }
}

impl Preview {
    pub fn begin(manager: &ITfThreadMgr, client: u32, snapshot: SelectionSnapshot) -> Result<Self> {
        if snapshot.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        // SAFETY: category manager only allocates an atom; no category registration.
        let categories: ITfCategoryMgr =
            unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
        let atom =
            unsafe { categories.RegisterGUID(&crate::display::GUID_INPUT_ATTRIBUTE)? } as i32;
        let preview = Self {
            manager: manager.clone(),
            client,
            snapshot: Rc::new(snapshot),
            atom,
            active: Rc::new(Cell::new(false)),
            composition: crate::owned_composition::OwnedComposition::new(),
        };
        let target = preview.snapshot.clone();
        let active = preview.active.clone();
        let composition = preview.composition.clone();
        let manager = manager.clone();
        run(
            &preview.snapshot.context,
            client,
            true,
            Box::new(move |cookie| {
                let range = validate_snapshot(&manager, &target, cookie)?;
                // SAFETY: fresh write cookie; preserve every pre-existing property run.
                unsafe {
                    let property = target.context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                    if runs(&property, &range, cookie)?
                        .iter()
                        .any(|(_, value)| !value.is_empty())
                    {
                        return Err(E_ABORT.into());
                    }
                    let range = validate_snapshot(&manager, &target, cookie)?;
                    property.SetValue(cookie, &range, &VARIANT::from(atom))?;
                    active.set(true);
                    composition.start(&target.context, &range, cookie)?;
                }
                Ok(())
            }),
        )?;
        Ok(preview)
    }

    fn check(&self) -> Result<()> {
        if self.snapshot.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        Ok(())
    }

    /// Cleanup uses property metadata only, even after permission/focus changes.
    /// A denied lock leaves the handle active so the owner can retry cleanup.
    pub fn cancel(&mut self) -> Result<()> {
        self.check()?;
        if !self.active.get() {
            return Ok(());
        }
        let target = self.snapshot.clone();
        let atom = self.atom;
        let composition = self.composition.clone();
        run(
            &target.context.clone(),
            self.client,
            true,
            Box::new(move |cookie| {
                composition.end(&target.context, cookie)?;
                // SAFETY: explicit original range, never a null/whole-context clear.
                unsafe {
                    let property = target.context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                    for (range, value) in runs(&property, &target.range, cookie)? {
                        if is_atom(&value, atom) {
                            property.Clear(cookie, &range)?;
                        }
                    }
                }
                Ok(())
            }),
        )?;
        self.active.set(false);
        Ok(())
    }

    pub(crate) fn context(&self) -> ITfContext {
        self.snapshot.context.clone()
    }

    /// Called only from a cleanup edit session for this exact context.
    pub(crate) fn clear_in_session(&mut self, cookie: u32) -> Result<()> {
        self.check()?;
        if !self.active.get() {
            return Ok(());
        }
        self.composition.end(&self.snapshot.context, cookie)?;
        // SAFETY: caller acquired a write cookie for context() on this thread.
        unsafe {
            let property = self.snapshot.context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
            for (range, value) in runs(&property, &self.snapshot.range, cookie)? {
                if is_atom(&value, self.atom) {
                    property.Clear(cookie, &range)?;
                }
            }
        }
        self.active.set(false);
        Ok(())
    }

    /// One guarded replacement. No fallible work follows a successful SetText.
    pub fn commit(&mut self, replacement: &str) -> Result<()> {
        self.check()?;
        if !self.active.get() {
            return Err(E_ABORT.into());
        }
        if !self.composition.can_commit() {
            return Err(E_ABORT.into());
        }
        let replacement: Vec<_> = replacement
            .encode_utf16()
            .take(MAX_SELECTION_UNITS + 1)
            .collect();
        if replacement.is_empty() || replacement.len() > MAX_SELECTION_UNITS {
            return Err(E_INVALIDARG.into());
        }
        let target = self.snapshot.clone();
        let manager = self.manager.clone();
        let atom = self.atom;
        let active = self.active.clone();
        let composition = self.composition.clone();
        run(
            &target.context.clone(),
            self.client,
            true,
            Box::new(move |cookie| {
                if !composition.can_commit() {
                    return Err(E_ABORT.into());
                }
                // Finish our metadata before using the ordinary idle-context
                // gate. No exception permits reading a foreign composition.
                composition.end(&target.context, cookie)?;
                if composition.was_terminated() {
                    return Err(E_ABORT.into());
                }
                let range = validate_snapshot(&manager, &target, cookie)?;
                // SAFETY: validated write session and owned attribute value.
                unsafe {
                    let property = target.context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                    if !is_atom(&property.GetValue(cookie, &range)?, atom) {
                        return Err(E_ABORT.into());
                    }
                    property.Clear(cookie, &range)?;
                    active.set(false);
                    let range = validate_snapshot(&manager, &target, cookie)?;
                    range.SetText(cookie, 0, &replacement)?;
                }
                Ok(())
            }),
        )
    }

    /// Synthetic-host diagnostic; never reads text or returns it to the caller.
    pub fn verify_attribute(&self, expected: bool) -> Result<()> {
        self.check()?;
        let target = self.snapshot.clone();
        let atom = self.atom;
        run(
            &target.context.clone(),
            self.client,
            false,
            Box::new(move |cookie| {
                // SAFETY: read cookie and retained same-context range.
                let value = unsafe {
                    target
                        .context
                        .GetProperty(&GUID_PROP_ATTRIBUTE)?
                        .GetValue(cookie, &target.range)?
                };
                if (expected && is_atom(&value, atom)) || (!expected && value.is_empty()) {
                    Ok(())
                } else {
                    Err(E_FAIL.into())
                }
            }),
        )
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        let _ = boundary(|| self.cancel());
    }
}
