//! Host-reported scope attributes, served through the real ITextStoreACP/TSF
//! app-property path. The fixture never calls SetInputScope on another window.
use super::*;
use windows::Win32::System::Com::CoTaskMemAlloc;
use windows::Win32::System::Variant::VARIANT;

#[derive(Clone)]
pub(super) enum ScopeSpec {
    List(Vec<InputScope>),
    Absent,
    WrongType,
    Failure,
    Null,
    Empty,
    OverLimit,
    AllocatedFailure,
}
#[implement(ITfInputScope)]
struct Scope {
    state: Rc<RefCell<ScopeSpec>>,
    queries: Rc<Cell<usize>>,
}
impl ITfInputScope_Impl for Scope_Impl {
    fn GetInputScopes(&self, output: *mut *mut InputScope, count: *mut u32) -> Result<()> {
        if output.is_null() || count.is_null() {
            return Err(E_INVALIDARG.into());
        }
        self.queries.set(self.queries.get().saturating_add(1));
        unsafe {
            output.write(std::ptr::null_mut());
            count.write(0);
        }
        let spec = self.state.borrow().clone();
        let values = match &spec {
            ScopeSpec::List(values) => values.clone(),
            ScopeSpec::Null => {
                unsafe {
                    count.write(1);
                }
                return Ok(());
            }
            ScopeSpec::Empty => vec![IS_TEXT],
            ScopeSpec::OverLimit => vec![IS_TEXT; 17],
            ScopeSpec::AllocatedFailure => vec![IS_TEXT],
            _ => return Err(E_FAIL.into()),
        };
        if values.is_empty() {
            return Ok(());
        }
        // SAFETY: caller frees the allocation with CoTaskMemFree; even the
        // deliberately failed/zero-count fixture transfers a valid allocation.
        unsafe {
            let pointer =
                CoTaskMemAlloc(std::mem::size_of_val(values.as_slice())).cast::<InputScope>();
            if pointer.is_null() {
                return Err(windows::Win32::Foundation::E_OUTOFMEMORY.into());
            }
            std::ptr::copy_nonoverlapping(values.as_ptr(), pointer, values.len());
            output.write(pointer);
            count.write(if matches!(spec, ScopeSpec::Empty) {
                0
            } else {
                values.len() as u32
            });
        }
        if matches!(spec, ScopeSpec::AllocatedFailure) {
            return Err(E_FAIL.into());
        }
        Ok(())
    }
    fn GetPhrase(&self, _: *mut *mut windows::core::BSTR, _: *mut u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetRegularExpression(&self) -> Result<windows::core::BSTR> {
        Err(E_NOTIMPL.into())
    }
    fn GetSRGS(&self) -> Result<windows::core::BSTR> {
        Err(E_NOTIMPL.into())
    }
    fn GetXML(&self) -> Result<windows::core::BSTR> {
        Err(E_NOTIMPL.into())
    }
}
pub(super) fn scope_value(
    state: &Rc<RefCell<ScopeSpec>>,
    queries: &Rc<Cell<usize>>,
) -> Result<Option<VARIANT>> {
    let mode = state.borrow().clone();
    match mode {
        ScopeSpec::Absent => Ok(None),
        ScopeSpec::WrongType => Ok(Some(VARIANT::from(1i32))),
        ScopeSpec::Failure => Err(E_FAIL.into()),
        _ => {
            let scope: ITfInputScope = Scope {
                state: state.clone(),
                queries: queries.clone(),
            }
            .into();
            Ok(Some(VARIANT::from(scope.cast::<IUnknown>()?)))
        }
    }
}
impl TextStore {
    pub(super) fn request_scope(&self, count: u32, attrs: *const GUID) -> Result<()> {
        self.scope_requested.set(false);
        if count > 64 || (count != 0 && attrs.is_null()) {
            return Err(E_INVALIDARG.into());
        }
        let requested = if count == 0 {
            true
        } else {
            // SAFETY: bounded caller-supplied filter array for this COM call.
            unsafe { std::slice::from_raw_parts(attrs, count as usize) }
                .contains(&GUID_PROP_INPUTSCOPE)
        };
        self.scope_requested.set(requested);
        Ok(())
    }
}

/// Only the fixture owns this context. Never set/clear these compartments in a
/// client to make an unknown application appear eligible for conversion.
pub(super) fn initialise_context(context: &ITfContext, client: u32) -> Result<()> {
    let compartments: ITfCompartmentMgr = context.cast()?;
    for guid in [
        GUID_COMPARTMENT_KEYBOARD_DISABLED,
        GUID_COMPARTMENT_EMPTYCONTEXT,
    ] {
        unsafe {
            compartments
                .GetCompartment(&guid)?
                .SetValue(client, &VARIANT::from(0i32))?;
        }
    }
    Ok(())
}

/// A real second input field for focus tests, prepared before capturing work.
pub(super) fn other_document(manager: &ITfThreadMgr, client: u32) -> Result<Document> {
    let document = Document(unsafe { manager.CreateDocumentMgr()? });
    let store = ComObject::new(TextStore::new("other"));
    let mut context = None;
    let mut cookie = 0;
    unsafe {
        document.0.CreateContext(
            client,
            0,
            &store.to_interface::<IUnknown>(),
            &mut context,
            &mut cookie,
        )?;
        let context = context.ok_or(E_FAIL)?;
        document.0.Push(&context)?;
        initialise_context(&context, client)?;
    }
    Ok(document)
}
