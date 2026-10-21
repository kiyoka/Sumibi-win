//! Positive metadata gate, not a guarantee that an application reports all
//! confidential fields. This module never calls text/phrase/grammar/XML APIs;
//! MSCTF may internally read its store while answering metadata queries.
use windows::core::{IUnknown, Interface, Result};
use windows::Win32::Foundation::{E_ACCESSDENIED, S_OK};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Variant::VT_I4;
use windows::Win32::UI::TextServices::*;

const MAX_SCOPES: usize = 16;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    dynamic: u32,
    statics: u32,
    scopes: [i32; MAX_SCOPES],
    count: usize,
}
fn denied<T>() -> Result<T> {
    Err(E_ACCESSDENIED.into())
}

fn classify(status: TS_STATUS, scopes: &[InputScope]) -> Result<Stamp> {
    let dynamic = TS_SD_READONLY | TS_SD_LOADING | TS_SD_RESERVED;
    let known_dynamic = dynamic
        | TS_SD_TKBAUTOCORRECTENABLE
        | TS_SD_TKBPREDICTIONENABLE
        | TS_SD_UIINTEGRATIONENABLE
        | TS_SD_INPUTPANEMANUALDISPLAYENABLE
        | TS_SD_EMBEDDEDHANDWRITINGVIEW_ENABLED
        | TS_SD_EMBEDDEDHANDWRITINGVIEW_VISIBLE;
    let supported_static =
        TS_SS_NOHIDDENTEXT | TS_SS_TKBAUTOCORRECTENABLE | TS_SS_TKBPREDICTIONENABLE;
    if status.dwDynamicFlags & dynamic != 0
        || status.dwDynamicFlags & !known_dynamic != 0
        || status.dwStaticFlags & TS_SS_NOHIDDENTEXT == 0
        || status.dwStaticFlags & !supported_static != 0
        || scopes.is_empty()
        || scopes.len() > MAX_SCOPES
    {
        return denied();
    }
    let mut stamp = Stamp {
        dynamic: status.dwDynamicFlags,
        statics: status.dwStaticFlags,
        scopes: [0; MAX_SCOPES],
        count: scopes.len(),
    };
    for (index, scope) in scopes.iter().enumerate() {
        // Default/absent and all unreviewed special-purpose scopes are denied.
        // This includes passwords, PINs, private, login, grammar and new values.
        if !matches!(*scope, IS_TEXT | IS_CHAT | IS_CHAT_WITHOUT_EMOJI) {
            return denied();
        }
        stamp.scopes[index] = scope.0;
    }
    stamp.scopes[..stamp.count].sort_unstable();
    Ok(stamp)
}
struct ScopeMemory(*mut InputScope);
impl Drop for ScopeMemory {
    fn drop(&mut self) {
        // SAFETY: GetInputScopes transfers a CoTaskMem allocation, including a
        // partially initialized failure. The interface caller contract applies.
        unsafe {
            CoTaskMemFree(Some(self.0.cast()));
        }
    }
}
pub(crate) fn check(context: &ITfContext, range: &ITfRange, cookie: u32) -> Result<Stamp> {
    check_with(context, range, cookie, || {
        crate::composition::require_idle(context)
    })
}
/// Only metadata inspection may admit the exact owned composition. All body
/// capture/source/write gates still call check(), which requires idle.
#[cfg(feature = "spike")]
pub(crate) fn owned_metadata(
    context: &ITfContext,
    range: &ITfRange,
    cookie: u32,
    owned: &crate::owned_composition::OwnedComposition,
) -> Result<Stamp> {
    check_with(context, range, cookie, || owned.require_exclusive(context))
}
fn check_with(
    context: &ITfContext,
    range: &ITfRange,
    cookie: u32,
    composition_gate: impl Fn() -> Result<()>,
) -> Result<Stamp> {
    // SAFETY: caller owns a current read/write cookie for this context/range.
    unsafe {
        let status = context.GetStatus()?;
        // Reject status before requesting even the scope object's metadata.
        classify(status, &[IS_TEXT])?;
        let compartments: ITfCompartmentMgr = context.cast()?;
        for guid in [
            GUID_COMPARTMENT_KEYBOARD_DISABLED,
            GUID_COMPARTMENT_EMPTYCONTEXT,
        ] {
            let value = compartments.GetCompartment(&guid)?.GetValue()?;
            if value.vt() != VT_I4 || value.Anonymous.Anonymous.Anonymous.lVal != 0 {
                return denied();
            }
        }
        // The convenience binding discards S_FALSE. Unsupported properties
        // must be rejected before calling a fallback property object's GetValue.
        let mut raw_property = std::ptr::null_mut();
        let status_property = (context.vtable().GetAppProperty)(
            context.as_raw(),
            &GUID_PROP_INPUTSCOPE,
            &mut raw_property,
        );
        let property = if raw_property.is_null() {
            None
        } else {
            Some(ITfReadOnlyProperty::from_raw(raw_property))
        };
        if status_property != S_OK {
            return denied();
        }
        let property = property.ok_or(E_ACCESSDENIED)?;
        // Missing, partial or mixed coverage returns VT_EMPTY and is rejected.
        let value = property.GetValue(cookie, range)?;
        let unknown = IUnknown::try_from(&value)?;
        let scope: ITfInputScope = unknown.cast()?;
        let mut memory = ScopeMemory(std::ptr::null_mut());
        let mut count = 0;
        if (scope.vtable().GetInputScopes)(scope.as_raw(), &mut memory.0, &mut count) != S_OK {
            return denied();
        }
        if memory.0.is_null() || count == 0 || count as usize > MAX_SCOPES {
            return denied();
        }
        let scopes = std::slice::from_raw_parts(memory.0, count as usize);
        let stamp = classify(status, scopes)?;
        composition_gate()?;
        // External metadata providers can reenter while returning their scope.
        let latest = context.GetStatus()?;
        if latest.dwDynamicFlags != status.dwDynamicFlags
            || latest.dwStaticFlags != status.dwStaticFlags
        {
            return denied();
        }
        for guid in [
            GUID_COMPARTMENT_KEYBOARD_DISABLED,
            GUID_COMPARTMENT_EMPTYCONTEXT,
        ] {
            let value = compartments.GetCompartment(&guid)?.GetValue()?;
            if value.vt() != VT_I4 || value.Anonymous.Anonymous.Anonymous.lVal != 0 {
                return denied();
            }
        }
        composition_gate()?;
        Ok(stamp)
    }
}
pub(crate) fn validate(
    context: &ITfContext,
    range: &ITfRange,
    cookie: u32,
    expected: Stamp,
) -> Result<()> {
    if check(context, range, cookie)? != expected {
        return denied();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status() -> TS_STATUS {
        TS_STATUS {
            dwDynamicFlags: 0,
            dwStaticFlags: TS_SS_NOHIDDENTEXT,
        }
    }
    #[test]
    fn only_reviewed_positive_scopes_are_accepted_in_every_combination() {
        for allowed in [IS_TEXT, IS_CHAT, IS_CHAT_WITHOUT_EMOJI] {
            assert!(classify(status(), &[allowed]).is_ok());
        }
        for raw in -8..=100 {
            let scope = InputScope(raw);
            if matches!(scope, IS_TEXT | IS_CHAT | IS_CHAT_WITHOUT_EMOJI) {
                continue;
            }
            assert!(classify(status(), &[scope]).is_err());
            assert!(classify(status(), &[IS_TEXT, scope]).is_err());
            assert!(classify(status(), &[scope, IS_TEXT]).is_err());
        }
    }
    #[test]
    fn status_and_scope_capacity_fail_closed() {
        for bit in 0..32 {
            let mut changed = status();
            changed.dwDynamicFlags = 1 << bit;
            if changed.dwDynamicFlags & (TS_SD_READONLY | TS_SD_LOADING | TS_SD_RESERVED) != 0
                || bit >= 9
            {
                assert!(classify(changed, &[IS_TEXT]).is_err());
            }
            changed = status();
            changed.dwStaticFlags |= 1 << bit;
            if !matches!(bit, 3..=5) {
                assert!(classify(changed, &[IS_TEXT]).is_err());
            }
        }
        assert!(classify(TS_STATUS::default(), &[IS_TEXT]).is_err());
        assert!(classify(status(), &[]).is_err());
        assert!(classify(status(), &[IS_TEXT; MAX_SCOPES + 1]).is_err());
    }
    #[test]
    fn metadata_revision_includes_scope_combination_and_status() {
        let first = classify(status(), &[IS_TEXT, IS_CHAT]).unwrap();
        assert!(first == classify(status(), &[IS_CHAT, IS_TEXT]).unwrap());
        assert!(first != classify(status(), &[IS_TEXT]).unwrap());
        let mut changed = status();
        changed.dwDynamicFlags |= TS_SD_UIINTEGRATIONENABLE;
        assert!(first != classify(changed, &[IS_TEXT, IS_CHAT]).unwrap());
    }
}
