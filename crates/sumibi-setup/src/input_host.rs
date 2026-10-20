//! Keyboard-only, single-line synthetic editor. Never attached to user documents.
use super::*;
use std::ops::Range;
use sumibi_tip::spike::{ISpikeControl, COMMAND_CONVERT, FUNCTION_TEST_CONTROL};
use windows::Win32::Foundation::{E_ACCESSDENIED, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::*;

impl TextStore {
    fn boundary(&self, position: usize) -> bool {
        let text = self.text.borrow();
        position <= text.len()
            && (position == 0
                || position == text.len()
                || !((0xd800..=0xdbff).contains(&text[position - 1])
                    && (0xdc00..=0xdfff).contains(&text[position])))
    }
    fn host_selection(&self, start: usize, end: usize) -> Result<()> {
        self.select_caret(start, end)
    }
    fn select_caret(&self, anchor: usize, caret: usize) -> Result<()> {
        if self.lock.get() != 0 {
            return Err(TS_E_NOLOCK.into());
        }
        if !self.boundary(anchor) || !self.boundary(caret) {
            return Err(E_INVALIDARG.into());
        }
        let mut selection = self.selection.get();
        selection.acpStart = anchor.min(caret) as i32;
        selection.acpEnd = anchor.max(caret) as i32;
        selection.style.ase = if caret < anchor {
            TS_AE_START
        } else {
            TS_AE_END
        };
        self.selection.set(selection);
        let sink = self.sink.borrow().clone();
        // SAFETY: host-originated selection change, no RefCell borrow held.
        if let Some(sink) = sink {
            unsafe {
                sink.OnSelectionChange()?;
            }
        }
        Ok(())
    }
    fn navigate(&self, key: usize, extend: bool, repeats: usize) -> Result<()> {
        let selected = self.selection.get();
        let start = selected.acpStart as usize;
        let end = selected.acpEnd as usize;
        let backwards = selected.style.ase == TS_AE_START;
        let mut caret = if backwards { start } else { end };
        let anchor = if backwards { end } else { start };
        let length = self.text.borrow().len();
        for step in 0..repeats.min(MAX_SELECTION_UNITS) {
            caret = match key {
                0x24 => 0,
                0x23 => length,
                0x25 if !extend && start != end && step == 0 => start,
                0x27 if !extend && start != end && step == 0 => end,
                0x25 => {
                    let mut next = caret.saturating_sub(1);
                    if !self.boundary(next) {
                        next -= 1;
                    }
                    next
                }
                0x27 => {
                    let mut next = (caret + 1).min(length);
                    if !self.boundary(next) {
                        next += 1;
                    }
                    next
                }
                _ => return Err(E_INVALIDARG.into()),
            };
        }
        self.select_caret(if extend { anchor } else { caret }, caret)
    }
    fn delete_forward(&self, repeats: usize) -> Result<()> {
        let selected = self.selection.get();
        let start = selected.acpStart as usize;
        let mut end = selected.acpEnd as usize;
        if start == end {
            let length = self.text.borrow().len();
            for _ in 0..repeats.min(MAX_SELECTION_UNITS) {
                if end == length {
                    break;
                }
                end += 1;
                if !self.boundary(end) {
                    end += 1;
                }
            }
        }
        if start == end {
            return Ok(());
        }
        self.user_edit(start..end, &[])
    }
    fn user_edit(&self, range: Range<usize>, replacement: &[u16]) -> Result<()> {
        if self.readonly.get() {
            return Err(E_ACCESSDENIED.into());
        }
        if self.lock.get() != 0 {
            return Err(TS_E_NOLOCK.into());
        }
        if range.start > range.end
            || !self.boundary(range.start)
            || !self.boundary(range.end)
            || String::from_utf16(replacement).is_err()
        {
            return Err(E_INVALIDARG.into());
        }
        let previous = self.text.borrow().clone();
        if previous.len() - range.len() + replacement.len() > MAX_SELECTION_UNITS {
            return Err(E_INVALIDARG.into());
        }
        let selection = self.selection.get();
        let mut next = previous.clone();
        next.splice(range.clone(), replacement.iter().copied());
        self.undo.borrow_mut().push((previous, selection));
        self.redo.borrow_mut().clear();
        *self.text.borrow_mut() = next;
        let caret = range.start + replacement.len();
        let mut selected = selection;
        selected.acpStart = caret as i32;
        selected.acpEnd = caret as i32;
        selected.style.ase = TS_AE_END;
        self.selection.set(selected);
        let sink = self.sink.borrow().clone();
        // SAFETY: host changed its own text outside a TSF lock. Notify only
        // after text and selection are consistent and all borrows are released.
        if let Some(sink) = sink {
            unsafe {
                sink.OnTextChange(
                    TS_ST_NONE,
                    &TS_TEXTCHANGE {
                        acpStart: range.start as i32,
                        acpOldEnd: range.end as i32,
                        acpNewEnd: caret as i32,
                    },
                )?;
                sink.OnSelectionChange()?;
            }
        }
        Ok(())
    }
    fn type_text(&self, text: &[u16]) -> Result<()> {
        let selected = self.selection.get();
        self.user_edit(selected.acpStart as usize..selected.acpEnd as usize, text)
    }
    fn backspace(&self) -> Result<()> {
        let selected = self.selection.get();
        let end = selected.acpEnd as usize;
        let mut start = selected.acpStart as usize;
        if start == end {
            if start == 0 {
                return Ok(());
            }
            start -= 1;
            if !self.boundary(start) {
                start -= 1;
            }
        }
        self.user_edit(start..end, &[])
    }
}

struct Editor {
    store: ComObject<TextStore>,
    context: ITfContext,
    keys: ITfKeystrokeMgr,
    control: ISpikeControl,
    high: Cell<Option<(u16, usize)>>,
    submits: Cell<usize>,
}
impl Editor {
    fn insert(&self, text: &[u16]) -> Result<()> {
        let selection = self.store.get().selection.get();
        let (start, end) = self
            .store
            .get()
            .bounds(selection.acpStart, selection.acpEnd)?;
        let length = self.store.get().text.borrow().len();
        if length - (end - start) + text.len() > MAX_SELECTION_UNITS {
            return Ok(());
        }
        self.store.get().type_text(text)
    }
    fn input(&self, message: u32, key: WPARAM, flags: LPARAM) -> Result<bool> {
        if message == WM_CLOSE {
            self.high.set(None);
            // SAFETY: revoke this editor's owner-thread grant before its view
            // is destroyed, so cleanup still has a live native input target.
            unsafe {
                self.control.RevokeContext().ok()?;
            }
            return Ok(false);
        }
        if message == WM_KILLFOCUS {
            self.high.set(None);
            return Ok(false);
        }
        if message == WM_KEYUP {
            // SAFETY: normal TSF application key routing on the owning thread.
            return Ok(unsafe {
                self.keys.TestKeyUp(key, flags).and_then(|test| {
                    if test.as_bool() {
                        self.keys.KeyUp(key, flags)
                    } else {
                        Ok(false.into())
                    }
                })
            }
            .map(|value| value.as_bool())
            .unwrap_or(false));
        }
        if message == WM_KEYDOWN {
            self.high.set(None);
            // SAFETY: test first; only invoke KeyDown when TSF requests it.
            let consumed = unsafe {
                self.keys.TestKeyDown(key, flags).and_then(|test| {
                    if test.as_bool() {
                        self.keys.KeyDown(key, flags)
                    } else {
                        Ok(false.into())
                    }
                })
            }
            .map(|value| value.as_bool())
            .unwrap_or(false);
            if consumed {
                return Ok(true);
            }
            if matches!(key.0, 0x23..=0x25 | 0x27 | 0x2e) {
                // SAFETY: read the calling thread's keyboard state; never modify it.
                let (shift, modified) = unsafe {
                    (
                        GetKeyState(VK_SHIFT.0 as i32) < 0,
                        GetKeyState(VK_CONTROL.0 as i32) < 0 || GetKeyState(VK_MENU.0 as i32) < 0,
                    )
                };
                if modified {
                    return Ok(false);
                }
                let repeats = (flags.0 as usize & 0xffff).max(1);
                if key.0 == 0x2e {
                    if !shift && !self.store.get().readonly.get() {
                        self.store.get().delete_forward(repeats)?;
                    }
                } else {
                    self.store.get().navigate(key.0, shift, repeats)?;
                }
                return Ok(true);
            }
            match key.0 {
                0x75 => {
                    // F6: explicitly grant only this editor's own context.
                    if self.store.get().text.borrow().is_empty() {
                        return Ok(true);
                    }
                    let selected = self.store.get().selection.get();
                    if selected.acpStart == selected.acpEnd {
                        let length = self.store.get().text.borrow().len();
                        self.store.get().host_selection(0, length)?;
                    }
                    unsafe {
                        self.control.GrantContext(self.context.as_raw()).ok()?;
                        self.control.BeginPreview().ok()?;
                    }
                    return Ok(true);
                }
                0x76 => {
                    // SAFETY: same-thread command for this editor's context;
                    // no keyboard event is sent to the desktop or other apps.
                    unsafe {
                        let _ = self
                            .keys
                            .SimulatePreservedKey(&self.context, &COMMAND_CONVERT)?;
                    }
                    return Ok(true);
                }
                _ => return Ok(false),
            }
        }
        if message != WM_CHAR {
            return Ok(false);
        }
        let Ok(unit) = u16::try_from(key.0) else {
            self.high.set(None);
            return Ok(true);
        };
        let repeats = (flags.0 as usize & 0xffff).max(1);
        if repeats > MAX_SELECTION_UNITS {
            self.high.set(None);
            return Ok(true);
        }
        if (0xd800..=0xdbff).contains(&unit) {
            self.high.set(Some((unit, repeats)));
            return Ok(true);
        }
        let previous = self.high.take();
        if self.store.get().readonly.get() {
            return Ok(true);
        }
        if (0xdc00..=0xdfff).contains(&unit) {
            if let Some((high, count)) = previous {
                if count == repeats && count <= MAX_SELECTION_UNITS / 2 {
                    self.insert(&[high, unit].repeat(count))?;
                }
            }
            return Ok(true);
        }
        match unit {
            0x01 => {
                let length = self.store.get().text.borrow().len();
                self.store.get().host_selection(0, length)?;
            }
            0x08 => {
                for _ in 0..repeats {
                    self.store.get().backspace()?;
                }
            }
            0x0d => {
                let end = self.store.get().selection.get().acpEnd as usize;
                self.store.get().host_selection(end, end)?;
                self.submits.set(self.submits.get().saturating_add(1));
            } // Single-line confirmation.
            0x19 => {
                if !self.store.get().redo.borrow().is_empty() {
                    self.store.get().redo_once()?;
                }
            }
            0x1a => {
                if !self.store.get().undo.borrow().is_empty() {
                    self.store.get().undo_once()?;
                }
            }
            0..=0x1f | 0x7f => {}
            _ => self.insert(&vec![unit; repeats])?,
        }
        Ok(true)
    }
}

pub(crate) fn exercise_input(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    run(manager, client, false)
}
pub(crate) fn run_demo(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    run(manager, client, true)
}

fn run(
    manager: &ITfThreadMgr,
    client: u32,
    interactive: bool,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    // SAFETY: this process owns the view, text store, apartment and all interfaces.
    unsafe {
        let window = crate::window_host::Window::new()?;
        window
            .set_title("Sumibi input demo | F6 preview | F7 convert | Ctrl+Z undo | Ctrl+Y redo")?;
        let store = ComObject::new(TextStore::new(""));
        store.get().window.set(window.hwnd());
        let document = Document(manager.CreateDocumentMgr()?);
        let mut context = None;
        let mut cookie = 0;
        document.0.CreateContext(
            client,
            0,
            &store.to_interface::<IUnknown>(),
            &mut context,
            &mut cookie,
        )?;
        let context = context.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
        document.0.Push(&context)?;
        let mut association = crate::window_host::Association::new(manager, &window, &document.0)?;
        window.focus()?;
        manager.SetFocus(&document.0)?;
        let control: ISpikeControl = manager
            .GetFunctionProvider(&sumibi_tip::CLSID_SUMIBI_TIP)?
            .GetFunction(&FUNCTION_TEST_CONTROL, &ISpikeControl::IID)?
            .cast()?;
        let editor = Rc::new(Editor {
            store: store.clone(),
            context: context.clone(),
            keys: manager.cast()?,
            control,
            high: Cell::new(None),
            submits: Cell::new(0),
        });
        let input = editor.clone();
        window.set_input(Rc::new(move |message, key, flags| {
            input.input(message, key, flags)
        }))?;
        let mut live = crate::live_render::Binding::attach(&window, &context, client)?;
        if interactive {
            println!("Type in the single-line demo. F6 previews, F7 converts, Enter confirms the original. Close the window to exit.");
            let mut message = MSG::default();
            while window.is_open() {
                let result = GetMessageW(&mut message, None, 0, 0).0;
                if result < 0 {
                    return Err(windows::core::Error::from_thread().into());
                }
                if result == 0 {
                    break;
                }
                window.route_message(&message)?;
            }
        } else {
            test_input(&window, &editor, &live)?;
        }
        window.clear_input();
        live.close()?;
        editor.control.RevokeContext().ok()?;
        association.close()?;
    }
    Ok(())
}

fn pump(window: &crate::window_host::Window) -> Result<()> {
    // SAFETY: process only this STA's queued messages, with a finite test bound.
    unsafe {
        let mut message = MSG::default();
        for _ in 0..512 {
            if !PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                return window.input_status();
            }
            window.route_message(&message)?;
        }
    }
    Err(E_FAIL.into())
}

fn test_input(
    window: &crate::window_host::Window,
    editor: &Editor,
    live: &crate::live_render::Binding,
) -> Result<()> {
    // SAFETY: fixture messages go only to our own window. No SendInput, global
    // keyboard-state changes, or messages to another application are used.
    let post = |message, key| unsafe {
        PostMessageW(Some(window.hwnd()), message, WPARAM(key), LPARAM(1))
    };
    pump(window)?;
    assert!(live.scene()?.text.is_empty());
    assert_eq!(live.scene()?.caret, 0);
    let _ = window.frame()?;
    for unit in "nihongo".encode_utf16() {
        post(WM_CHAR, unit as usize)?;
    }
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "nihongo".encode_utf16().collect::<Vec<_>>()
    );
    post(WM_CHAR, 0x08)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "nihong".encode_utf16().collect::<Vec<_>>()
    );
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "nihongo".encode_utf16().collect::<Vec<_>>()
    );
    println!("WM_CHAR typing / Backspace / undo / automatic redraw: OK");
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    assert_eq!(live.scene()?.underlines.len(), 1);
    post(WM_KEYDOWN, 0x1b)?;
    pump(window)?;
    assert!(live.scene()?.underlines.is_empty());
    assert_eq!(editor.submits.get(), 0);
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    post(WM_KEYDOWN, 0x0d)?;
    pump(window)?;
    assert_eq!(editor.submits.get(), 1);
    assert_eq!(editor.store.get().selection.get().acpStart, 7);
    assert_eq!(editor.store.get().selection.get().acpEnd, 7);
    assert_eq!(
        *editor.store.get().text.borrow(),
        "nihongo".encode_utf16().collect::<Vec<_>>()
    );
    assert!(live.scene()?.underlines.is_empty());
    println!("Native message loop: Esc consumed; Enter confirms once without conversion: OK");
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    post(WM_KEYDOWN, 0x76)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "日本語".encode_utf16().collect::<Vec<_>>()
    );
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "nihongo".encode_utf16().collect::<Vec<_>>()
    );
    println!("Native input loop -> preview -> fixed conversion -> one-step undo: OK");
    post(WM_CHAR, 0x01)?;
    for unit in "😀".encode_utf16() {
        post(WM_CHAR, unit as usize)?;
    }
    pump(window)?;
    assert_eq!(live.scene()?.text, "😀".encode_utf16().collect::<Vec<_>>());
    post(WM_CHAR, 0x08)?;
    pump(window)?;
    assert!(editor.store.get().text.borrow().is_empty());
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, "😀".encode_utf16().collect::<Vec<_>>());
    post(WM_CHAR, 0xdc00)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, "😀".encode_utf16().collect::<Vec<_>>());
    editor.store.get().readonly.set(true);
    post(WM_CHAR, 'x' as usize)?;
    post(WM_CHAR, 0x08)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, "😀".encode_utf16().collect::<Vec<_>>());
    editor.store.get().readonly.set(false);
    println!("Surrogate pair input / scalar Backspace / invalid low surrogate / read-only: OK");
    post(WM_KEYDOWN, 0x24)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 0);
    post(WM_KEYDOWN, 0x23)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 2);
    assert!(editor.store.get().user_edit(1..2, &[]).is_err());
    let before = editor.store.get().text.borrow().clone();
    assert!(editor
        .store
        .get()
        .user_edit(0..0, &vec![b'x' as u16; MAX_SELECTION_UNITS + 1])
        .is_err());
    assert_eq!(*editor.store.get().text.borrow(), before);
    println!("Home/End caret, surrogate boundary and size limit guards: OK");
    post(WM_CHAR, 0x01)?;
    // A single WM_CHAR can represent multiple repeated characters.
    unsafe {
        PostMessageW(
            Some(window.hwnd()),
            WM_CHAR,
            WPARAM('a' as usize),
            LPARAM(3),
        )?;
    }
    pump(window)?;
    assert_eq!(live.scene()?.text, "aaa".encode_utf16().collect::<Vec<_>>());
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, before);
    println!("WM_CHAR repeat count applies once and undoes as one edit: OK");
    post(WM_KEYDOWN, 0x24)?;
    post(WM_KEYDOWN, 0x27)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 2);
    post(WM_KEYDOWN, 0x25)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 0);
    editor.store.get().readonly.set(true);
    post(WM_KEYDOWN, 0x2e)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, before);
    editor.store.get().readonly.set(false);
    post(WM_KEYDOWN, 0x2e)?;
    pump(window)?;
    assert!(live.scene()?.text.is_empty());
    let undo_count = editor.store.get().undo.borrow().len();
    post(WM_KEYDOWN, 0x2e)?;
    pump(window)?;
    assert_eq!(editor.store.get().undo.borrow().len(), undo_count);
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, before);
    println!("Native Left/Right and Delete preserve surrogate pairs; read-only and no-op undo: OK");

    // Exercise Shift semantics explicitly without altering the desktop keyboard
    // state. Plain navigation/Delete above still travel through the message loop.
    editor.store.get().host_selection(2, 2)?;
    editor.store.get().navigate(0x25, true, 1)?;
    pump(window)?;
    assert_eq!(live.scene()?.selection, 0..2);
    assert_eq!(live.scene()?.caret, 0);
    editor.store.get().navigate(0x27, true, 1)?;
    pump(window)?;
    assert_eq!(live.scene()?.selection, 2..2);
    assert_eq!(live.scene()?.caret, 2);

    editor.store.get().host_selection(0, 2)?;
    editor.insert(&"a😀bc".encode_utf16().collect::<Vec<_>>())?;
    editor.store.get().host_selection(3, 3)?;
    editor.store.get().navigate(0x25, true, 1)?;
    pump(window)?;
    assert_eq!(live.scene()?.selection, 1..3);
    assert_eq!(live.scene()?.caret, 1);
    editor.store.get().navigate(0x23, true, 1)?;
    pump(window)?;
    assert_eq!(live.scene()?.selection, 3..5);
    assert_eq!(live.scene()?.caret, 5);
    editor.store.get().navigate(0x24, true, 1)?;
    pump(window)?;
    assert_eq!(live.scene()?.selection, 0..3);
    assert_eq!(live.scene()?.caret, 0);
    post(WM_KEYDOWN, 0x27)?;
    pump(window)?;
    assert_eq!(live.scene()?.selection, 3..3);
    editor.store.get().navigate(0x25, true, 1)?;
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    assert_eq!(live.scene()?.underlines, vec![1..3]);
    post(WM_KEYDOWN, 0x25)?;
    pump(window)?;
    assert!(live.scene()?.underlines.is_empty());
    assert_eq!(live.scene()?.caret, 1);
    editor.store.get().host_selection(3, 3)?;
    editor.store.get().navigate(0x25, true, 1)?;
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    post(WM_KEYDOWN, 0x76)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "a日本語bc".encode_utf16().collect::<Vec<_>>()
    );
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "a😀bc".encode_utf16().collect::<Vec<_>>()
    );
    assert_eq!(live.scene()?.selection, 1..3);
    assert_eq!(live.scene()?.caret, 1);
    post(WM_KEYDOWN, 0x2e)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, "abc".encode_utf16().collect::<Vec<_>>());
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 1);
    println!("Shift selection direction / anchor crossing / selected conversion / cancellation / Delete / undo: OK");
    post(WM_CHAR, 0x19)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, "abc".encode_utf16().collect::<Vec<_>>());
    assert_eq!(live.scene()?.selection, 1..1);
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 1);
    assert_eq!(live.scene()?.selection, 1..3);
    let counts = (
        editor.store.get().undo.borrow().len(),
        editor.store.get().redo.borrow().len(),
    );
    editor.store.get().readonly.set(true);
    post(WM_CHAR, 0x19)?;
    pump(window)?;
    assert!(editor.store.get().redo_once().is_err());
    assert!(editor.store.get().undo_once().is_err());
    editor.store.get().readonly.set(false);
    editor.store.get().lock.set(TS_LF_READ.0);
    assert!(editor.store.get().redo_once().is_err());
    assert!(editor.store.get().undo_once().is_err());
    editor.store.get().lock.set(0);
    assert_eq!(
        counts,
        (
            editor.store.get().undo.borrow().len(),
            editor.store.get().redo.borrow().len()
        )
    );
    assert_eq!(
        live.scene()?.text,
        "a😀bc".encode_utf16().collect::<Vec<_>>()
    );
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    post(WM_CHAR, 0x19)?;
    pump(window)?;
    assert_eq!(live.scene()?.text, "abc".encode_utf16().collect::<Vec<_>>());
    assert!(live.scene()?.underlines.is_empty());
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    post(WM_KEYDOWN, 0x75)?;
    pump(window)?;
    post(WM_KEYDOWN, 0x76)?;
    pump(window)?;
    assert!(editor.store.get().redo.borrow().is_empty());
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    assert_eq!(live.scene()?.caret, 1);
    post(WM_CHAR, 0x19)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "a日本語bc".encode_utf16().collect::<Vec<_>>()
    );
    assert!(live.scene()?.underlines.is_empty());
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    post(WM_CHAR, 'x' as usize)?;
    pump(window)?;
    assert!(editor.store.get().redo.borrow().is_empty());
    post(WM_CHAR, 0x19)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "axbc".encode_utf16().collect::<Vec<_>>()
    );
    post(WM_CHAR, 0x1a)?;
    pump(window)?;
    post(WM_CHAR, 0x19)?;
    pump(window)?;
    assert_eq!(
        live.scene()?.text,
        "axbc".encode_utf16().collect::<Vec<_>>()
    );
    println!("Redo typing / Delete / conversion; selection restored; stale branch discarded; lock/read-only guards; preview invalidation: OK");
    post(WM_CLOSE, 0)?;
    pump(window)?;
    assert!(!window.is_open());
    println!("WM_CLOSE revokes the grant and closes the native input view: OK");
    window.input_status()
}
