//! Positive metadata gate, not a guarantee that an application reports all
//! confidential fields. This module never calls text/phrase/grammar/XML APIs;
//! MSCTF may internally read its store while answering metadata queries.
use windows::core::{IUnknown, Interface, Result};
use windows::Win32::Foundation::{E_ACCESSDENIED, S_OK};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Variant::VT_I4;
use windows::Win32::UI::TextServices::*;

const MAX_SCOPES: usize = 16;

/// Fixed metadata-only diagnostic codes. These carry no document text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputStatusDenial {
    ReadOnly,
    Loading,
    ReservedDynamicFlags,
    UnknownDynamicFlags,
    HiddenTextPossible,
    MultipleSelections,
    MultipleRegions,
    TransitoryContext,
    UnknownStaticFlags,
}

/// Explain the first unsupported document-status condition. `None` means only
/// that status passed; scopes, compartments, focus, selection and composition
/// still require the ordinary gate. This never grants permission to read/edit.
pub fn input_status_denial(status: TS_STATUS) -> Option<InputStatusDenial> {
    use InputStatusDenial::*;
    let known_dynamic = TS_SD_READONLY
        | TS_SD_LOADING
        | TS_SD_RESERVED
        | TS_SD_TKBAUTOCORRECTENABLE
        | TS_SD_TKBPREDICTIONENABLE
        | TS_SD_UIINTEGRATIONENABLE
        | TS_SD_INPUTPANEMANUALDISPLAYENABLE
        | TS_SD_EMBEDDEDHANDWRITINGVIEW_ENABLED
        | TS_SD_EMBEDDEDHANDWRITINGVIEW_VISIBLE;
    let supported_static =
        TS_SS_NOHIDDENTEXT | TS_SS_TKBAUTOCORRECTENABLE | TS_SS_TKBPREDICTIONENABLE;
    if status.dwDynamicFlags & TS_SD_READONLY != 0 {
        Some(ReadOnly)
    } else if status.dwDynamicFlags & TS_SD_LOADING != 0 {
        Some(Loading)
    } else if status.dwDynamicFlags & TS_SD_RESERVED != 0 {
        Some(ReservedDynamicFlags)
    } else if status.dwDynamicFlags & !known_dynamic != 0 {
        Some(UnknownDynamicFlags)
    } else if status.dwStaticFlags & TS_SS_NOHIDDENTEXT == 0 {
        Some(HiddenTextPossible)
    } else if status.dwStaticFlags & TS_SS_DISJOINTSEL != 0 {
        Some(MultipleSelections)
    } else if status.dwStaticFlags & TS_SS_REGIONS != 0 {
        Some(MultipleRegions)
    } else if status.dwStaticFlags & TS_SS_TRANSITORY != 0 {
        Some(TransitoryContext)
    } else if status.dwStaticFlags & !supported_static != 0 {
        Some(UnknownStaticFlags)
    } else {
        None
    }
}

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
    if input_status_denial(status).is_some() || scopes.is_empty() || scopes.len() > MAX_SCOPES {
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
/// Metadata inspection may admit the exact owned composition. This check alone
/// never authorizes body access. Capture/source/result application require idle;
/// the diagnostic input writer separately validates supplied text and an owned
/// insertion range, and never reads existing body text.
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

    #[test]
    fn status_diagnostics_name_blockers_without_authorizing_protected_scopes() {
        use InputStatusDenial::*;
        for (dynamic, statics, expected) in [
            (TS_SD_READONLY, TS_SS_NOHIDDENTEXT, ReadOnly),
            (TS_SD_LOADING, TS_SS_NOHIDDENTEXT, Loading),
            (TS_SD_RESERVED, TS_SS_NOHIDDENTEXT, ReservedDynamicFlags),
            (1 << 31, TS_SS_NOHIDDENTEXT, UnknownDynamicFlags),
            (0, TS_SS_REGIONS, HiddenTextPossible),
            (
                0,
                TS_SS_NOHIDDENTEXT | TS_SS_DISJOINTSEL,
                MultipleSelections,
            ),
            (0, TS_SS_NOHIDDENTEXT | TS_SS_REGIONS, MultipleRegions),
            (0, TS_SS_NOHIDDENTEXT | TS_SS_TRANSITORY, TransitoryContext),
            (0, TS_SS_NOHIDDENTEXT | (1 << 31), UnknownStaticFlags),
        ] {
            let status = TS_STATUS {
                dwDynamicFlags: dynamic,
                dwStaticFlags: statics,
            };
            assert_eq!(input_status_denial(status), Some(expected));
            assert!(classify(status, &[IS_TEXT]).is_err());
        }
        assert_eq!(input_status_denial(status()), None);
        for scope in [IS_PASSWORD, IS_PRIVATE, IS_DEFAULT] {
            assert!(classify(status(), &[scope]).is_err());
        }
    }
}
