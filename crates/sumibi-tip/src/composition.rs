//! Until Sumibi owns a tracked composition, any composition in this context
//! makes the selection conversion unavailable. Never inspect its body, take
//! ownership, or terminate another service's composition.
use windows::core::{Interface, Result};
use windows::Win32::Foundation::{E_ACCESSDENIED, S_FALSE, S_OK};
use windows::Win32::UI::TextServices::{
    IEnumITfCompositionView, ITfCompositionView, ITfContext, ITfContextComposition,
};

pub(crate) fn require_idle(context: &ITfContext) -> Result<()> {
    let compositions: ITfContextComposition = context.cast()?;
    // The generated convenience binding discards S_FALSE. Keep exact HRESULTs
    // and release even a partially initialized output on any rejected status.
    let mut raw = std::ptr::null_mut();
    // SAFETY: live context interface on the caller's owning TSF thread, with an
    // initialized output slot. No view/range or cookie escapes this call.
    let status =
        unsafe { (compositions.vtable().EnumCompositions)(compositions.as_raw(), &mut raw) };
    let enumerator = if raw.is_null() {
        None
    } else {
        // SAFETY: EnumCompositions returns one owned COM reference.
        Some(unsafe { IEnumITfCompositionView::from_raw(raw) })
    };
    if status != S_OK {
        return Err(E_ACCESSDENIED.into());
    }
    require_empty(&enumerator.ok_or(E_ACCESSDENIED)?)
}

fn require_empty(enumerator: &IEnumITfCompositionView) -> Result<()> {
    let mut view: Option<ITfCompositionView> = None;
    let mut fetched = 0;
    // SAFETY: one initialized output slot, including partial-error cleanup.
    // One query suffices; neither a hostile count nor a long list causes a loop.
    let status = unsafe {
        (enumerator.vtable().Next)(
            enumerator.as_raw(),
            1,
            (&mut view as *mut Option<ITfCompositionView>).cast(),
            &mut fetched,
        )
    };
    if status != S_FALSE || fetched != 0 || view.is_some() {
        return Err(E_ACCESSDENIED.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::rc::Rc;
    use windows::core::{implement, IUnknown_Vtbl, GUID, HRESULT};
    use windows::Win32::Foundation::{E_FAIL, E_NOINTERFACE, E_NOTIMPL};
    use windows::Win32::UI::TextServices::{
        IEnumITfCompositionView_Vtbl, ITfCompositionView_Impl, ITfRange,
    };

    #[implement(ITfCompositionView)]
    struct View(Rc<Cell<u32>>);
    impl Drop for View {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    impl ITfCompositionView_Impl for View_Impl {
        fn GetOwnerClsid(&self) -> Result<GUID> {
            panic!("composition owner must not be queried")
        }
        fn GetRange(&self) -> Result<ITfRange> {
            panic!("composition range must not be queried")
        }
    }

    // Raw test provider so S_FALSE and inconsistent success/error outputs reach
    // the real COM adapter unchanged by generated Result<()> implementations.
    #[repr(C)]
    struct Enumerator {
        vtable: *const IEnumITfCompositionView_Vtbl,
        status: HRESULT,
        fetched: u32,
        view: RefCellView,
        calls: Cell<u32>,
    }
    type RefCellView = std::cell::RefCell<Option<ITfCompositionView>>;
    unsafe extern "system" fn query(
        _: *mut c_void,
        _: *const GUID,
        output: *mut *mut c_void,
    ) -> HRESULT {
        unsafe { output.write(std::ptr::null_mut()) };
        E_NOINTERFACE
    }
    unsafe extern "system" fn refs(_: *mut c_void) -> u32 {
        1
    }
    unsafe extern "system" fn clone_enum(_: *mut c_void, _: *mut *mut c_void) -> HRESULT {
        E_NOTIMPL
    }
    unsafe extern "system" fn reset(_: *mut c_void) -> HRESULT {
        E_NOTIMPL
    }
    unsafe extern "system" fn skip(_: *mut c_void, _: u32) -> HRESULT {
        E_NOTIMPL
    }
    unsafe extern "system" fn next(
        this: *mut c_void,
        count: u32,
        output: *mut *mut c_void,
        fetched: *mut u32,
    ) -> HRESULT {
        let this = unsafe { &*(this as *const Enumerator) };
        assert_eq!(count, 1);
        this.calls.set(this.calls.get() + 1);
        unsafe {
            output.write(
                this.view
                    .borrow_mut()
                    .take()
                    .map_or(std::ptr::null_mut(), Interface::into_raw),
            );
            fetched.write(this.fetched);
        }
        this.status
    }
    static VTABLE: IEnumITfCompositionView_Vtbl = IEnumITfCompositionView_Vtbl {
        base__: IUnknown_Vtbl {
            QueryInterface: query,
            AddRef: refs,
            Release: refs,
        },
        Clone: clone_enum,
        Next: next,
        Reset: reset,
        Skip: skip,
    };

    #[test]
    fn exact_empty_result_only_and_partial_outputs_released() {
        for status in [S_FALSE, S_OK, E_FAIL, HRESULT(2)] {
            for fetched in [0, 1, 2, u32::MAX] {
                for has_view in [false, true] {
                    let drops = Rc::new(Cell::new(0));
                    let view = has_view.then(|| ITfCompositionView::from(View(drops.clone())));
                    let mut provider = Enumerator {
                        vtable: &VTABLE,
                        status,
                        fetched,
                        view: std::cell::RefCell::new(view),
                        calls: Cell::new(0),
                    };
                    let raw = (&mut provider as *mut Enumerator).cast::<c_void>();
                    // SAFETY: stack provider and its vtable outlive this borrowed interface.
                    let enumerator =
                        unsafe { IEnumITfCompositionView::from_raw_borrowed(&raw).unwrap() };
                    assert_eq!(
                        require_empty(enumerator).is_ok(),
                        status == S_FALSE && fetched == 0 && !has_view
                    );
                    assert_eq!(provider.calls.get(), 1);
                    assert_eq!(drops.get(), u32::from(has_view));
                }
            }
        }
    }
}
