//! Read and render only the synthetic host's document. No production UI yet.
#![allow(non_snake_case)]
use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::ops::Range;
use std::rc::Rc;
use std::thread::{self, ThreadId};
use sumibi_tip::edit::MAX_SELECTION_UNITS;
use windows::core::{implement, Interface, Result};
use windows::Win32::Foundation::{COLORREF, E_ABORT, E_FAIL, RPC_E_WRONG_THREAD, SIZE, S_OK};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::VT_I4;
use windows::Win32::UI::TextServices::*;

pub(crate) const WIDTH: usize = 640;
pub(crate) const HEIGHT: usize = 160;
const TEXT_X: i32 = 24;
const TEXT_Y: i32 = 40;

#[derive(Default)]
pub(crate) struct Scene {
    pub text: Vec<u16>,
    pub underlines: Vec<Range<usize>>,
    pub selection: Range<usize>,
    pub caret: usize,
}

struct Selection(TF_SELECTION);
impl Drop for Selection {
    fn drop(&mut self) {
        // SAFETY: GetSelection transfers an owned range even on partial failure.
        unsafe {
            ManuallyDrop::drop(&mut self.0.range);
        }
    }
}

#[implement(ITfEditSession)]
struct ReadScene {
    context: ITfContext,
    provider: ITfDisplayAttributeProvider,
    categories: ITfCategoryMgr,
    output: Rc<RefCell<Option<Scene>>>,
    active: Rc<Cell<bool>>,
    owner: ThreadId,
}
impl ITfEditSession_Impl for ReadScene_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if self.owner != thread::current().id() {
                return Err(RPC_E_WRONG_THREAD.into());
            }
            if !self.active.replace(false) {
                return Err(E_ABORT.into());
            }
            // SAFETY: all property/text reads use this context's read cookie.
            unsafe {
                let full = self.context.GetStart(cookie)?;
                full.ShiftEndToRange(cookie, &self.context.GetEnd(cookie)?, TF_ANCHOR_END)?;
                let mut start = 0;
                let mut length = 0;
                full.cast::<ITfRangeACP>()?
                    .GetExtent(&mut start, &mut length)?;
                if start != 0 || length < 0 || length as usize > MAX_SELECTION_UNITS {
                    return Err(E_ABORT.into());
                }
                let mut text = vec![0; length as usize];
                let mut fetched = 0;
                full.GetText(cookie, 0, &mut text, &mut fetched)?;
                if fetched != length as u32 || String::from_utf16(&text).is_err() {
                    return Err(E_ABORT.into());
                }
                let mut scene = Scene {
                    text,
                    underlines: Vec::new(),
                    selection: 0..0,
                    caret: 0,
                };
                let mut selection = Selection(TF_SELECTION::default());
                let mut count = 0;
                self.context.GetSelection(
                    cookie,
                    TF_DEFAULT_SELECTION,
                    std::slice::from_mut(&mut selection.0),
                    &mut count,
                )?;
                if count != 1 {
                    return Err(E_ABORT.into());
                }
                let selected = selection.0.range.take().ok_or(E_FAIL)?;
                let mut anchor = 0;
                let mut selected_length = 0;
                selected
                    .cast::<ITfRangeACP>()?
                    .GetExtent(&mut anchor, &mut selected_length)?;
                if anchor < 0
                    || selected_length < 0
                    || anchor as usize + selected_length as usize > scene.text.len()
                {
                    return Err(E_ABORT.into());
                }
                scene.selection = anchor as usize..(anchor + selected_length) as usize;
                scene.caret = if selection.0.style.ase == TF_AE_START {
                    scene.selection.start
                } else {
                    scene.selection.end
                };
                if scene.text.is_empty() {
                    *self.output.borrow_mut() = Some(scene);
                    return Ok(());
                }
                let property = self.context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                let mut enumerator = None;
                property.EnumRanges(cookie, &mut enumerator, &full)?;
                let enumerator = enumerator.ok_or(E_FAIL)?;
                let mut inspected = 0;
                loop {
                    let mut ranges = [None];
                    let mut count = 0;
                    enumerator.Next(&mut ranges, &mut count)?;
                    if count == 0 {
                        break;
                    }
                    inspected += 1;
                    if count != 1 || inspected > MAX_SELECTION_UNITS * 2 + 1 {
                        return Err(E_ABORT.into());
                    }
                    let range = ranges[0].take().ok_or(E_FAIL)?;
                    let value = property.GetValue(cookie, &range)?;
                    if value.is_empty() {
                        continue;
                    }
                    if value.vt() != VT_I4 {
                        return Err(E_FAIL.into());
                    }
                    let guid = self
                        .categories
                        .GetGUID(value.Anonymous.Anonymous.Anonymous.lVal as u32)?;
                    let info = self.provider.GetDisplayAttributeInfo(&guid)?;
                    let mut style = TF_DISPLAYATTRIBUTE::default();
                    info.GetAttributeInfo(&mut style)?;
                    // Deliberately support only this spike's fixed definition.
                    // Other providers/styles need a real text layout engine.
                    if style.lsStyle != TF_LS_DOT
                        || style.crText.r#type != TF_CT_NONE
                        || style.crBk.r#type != TF_CT_NONE
                        || style.fBoldLine.as_bool()
                    {
                        return Err(E_FAIL.into());
                    }
                    let mut start = 0;
                    let mut length = 0;
                    range
                        .cast::<ITfRangeACP>()?
                        .GetExtent(&mut start, &mut length)?;
                    if start < 0
                        || length <= 0
                        || (start as usize + length as usize) > scene.text.len()
                    {
                        return Err(E_ABORT.into());
                    }
                    scene
                        .underlines
                        .push(start as usize..(start + length) as usize);
                }
                *self.output.borrow_mut() = Some(scene);
            }
            Ok(())
        }))
        .unwrap_or_else(|_| Err(E_FAIL.into()))
    }
}

pub(crate) fn capture(context: &ITfContext, client: u32) -> Result<Scene> {
    let output = Rc::new(RefCell::new(None));
    let active = Rc::new(Cell::new(true));
    // SAFETY: process-local class factory supplies our DLL's provider. This
    // bypasses persistent category registration, not property/attribute lookup.
    let session: ITfEditSession = unsafe {
        ReadScene {
            context: context.clone(),
            provider: CoCreateInstance(&sumibi_tip::CLSID_SUMIBI_TIP, None, CLSCTX_INPROC_SERVER)?,
            categories: CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?,
            output: output.clone(),
            active: active.clone(),
            owner: thread::current().id(),
        }
        .into()
    };
    // SAFETY: only a synchronous read is accepted. A late callback is disabled.
    let requested =
        unsafe { context.RequestEditSession(client, &session, TF_ES_SYNC | TF_ES_READ) };
    active.set(false);
    let status = requested?;
    if status != S_OK {
        return Err(if status.is_err() { status } else { E_FAIL }.into());
    }
    let scene = output.borrow_mut().take().ok_or(E_FAIL)?;
    Ok(scene)
}

pub(crate) fn bitmap_info() -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: WIDTH as i32,
            biHeight: -(HEIGHT as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}
impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: restore before deleting the owned bitmap and compatible DC.
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

pub(crate) struct Frame {
    pub pixels: Vec<u8>,
    pub line_y: usize,
    pub spans: Vec<Range<usize>>,
}
impl Frame {
    pub(crate) fn draw(scene: &Scene, label: &str) -> Result<Self> {
        // SAFETY: all drawing targets a private, bounded, top-down 32-bit DIB.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.0.is_null() {
                return Err(E_FAIL.into());
            }
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(
                Some(dc),
                &bitmap_info(),
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            ) {
                Ok(bitmap) => bitmap,
                Err(error) => {
                    let _ = DeleteDC(dc);
                    return Err(error);
                }
            };
            let previous = SelectObject(dc, bitmap.into());
            let surface = Surface {
                dc,
                bitmap,
                previous,
            };
            if bits.is_null() || previous.0.is_null() {
                return Err(E_FAIL.into());
            }
            let rect = windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: WIDTH as i32,
                bottom: HEIGHT as i32,
            };
            if FillRect(dc, &rect, HBRUSH(GetStockObject(WHITE_BRUSH).0)) == 0 {
                return Err(E_FAIL.into());
            }
            let saved = SaveDC(dc);
            if saved == 0 {
                return Err(E_FAIL.into());
            }
            SelectObject(dc, GetStockObject(DEFAULT_GUI_FONT));
            SetTextColor(dc, COLORREF(0));
            SetBkMode(dc, TRANSPARENT);
            let rendered = (|| -> Result<(usize, Vec<Range<usize>>)> {
                if scene.selection.start > scene.selection.end
                    || scene.selection.end > scene.text.len()
                    || scene.caret > scene.text.len()
                {
                    return Err(E_FAIL.into());
                }
                let mut metrics = TEXTMETRICW::default();
                if !GetTextMetricsW(dc, &mut metrics).as_bool() {
                    return Err(E_FAIL.into());
                }
                let mut size = SIZE::default();
                if !GetTextExtentPoint32W(dc, &scene.text, &mut size).as_bool() {
                    return Err(E_FAIL.into());
                }
                let height = size.cy.max(metrics.tmHeight);
                let width_at = |position: usize| -> Result<i32> {
                    let mut extent = SIZE::default();
                    if !GetTextExtentPoint32W(dc, &scene.text[..position], &mut extent).as_bool() {
                        return Err(E_FAIL.into());
                    }
                    Ok(TEXT_X + extent.cx)
                };
                if !scene.selection.is_empty() {
                    let rectangle = windows::Win32::Foundation::RECT {
                        left: width_at(scene.selection.start)?,
                        top: TEXT_Y,
                        right: width_at(scene.selection.end)?,
                        bottom: TEXT_Y + height,
                    };
                    let brush = CreateSolidBrush(COLORREF(0xffe3c7));
                    if brush.0.is_null() {
                        return Err(E_FAIL.into());
                    }
                    let filled = FillRect(dc, &rectangle, brush);
                    let _ = DeleteObject(brush.into());
                    if filled == 0 {
                        return Err(E_FAIL.into());
                    }
                }
                if !TextOutW(dc, TEXT_X, TEXT_Y, &scene.text).as_bool() {
                    return Err(E_FAIL.into());
                }
                let caret_x = width_at(scene.caret)?;
                if (0..WIDTH as i32).contains(&caret_x) {
                    for y in TEXT_Y..TEXT_Y + height {
                        if SetPixel(dc, caret_x, y, COLORREF(0)).0 == CLR_INVALID {
                            return Err(E_FAIL.into());
                        }
                    }
                }
                let line_y = TEXT_Y + height + 2;
                if line_y < 0 || line_y as usize >= HEIGHT {
                    return Err(E_FAIL.into());
                }
                let mut spans = Vec::new();
                for range in &scene.underlines {
                    if range.start >= range.end || range.end > scene.text.len() {
                        return Err(E_FAIL.into());
                    }
                    let mut left = SIZE::default();
                    let mut right = SIZE::default();
                    if !GetTextExtentPoint32W(dc, &scene.text[..range.start], &mut left).as_bool()
                        || !GetTextExtentPoint32W(dc, &scene.text[..range.end], &mut right)
                            .as_bool()
                    {
                        return Err(E_FAIL.into());
                    }
                    let x1 = TEXT_X + left.cx;
                    let x2 = TEXT_X + right.cx;
                    if x1 < 0 || x2 <= x1 || x2 as usize >= WIDTH {
                        return Err(E_FAIL.into());
                    }
                    // Dots are aligned in device pixels; gaps retain background.
                    for x in (x1..x2).step_by(2) {
                        if SetPixel(dc, x, line_y, COLORREF(0)).0 == CLR_INVALID {
                            return Err(E_FAIL.into());
                        }
                    }
                    spans.push(x1 as usize..x2 as usize);
                }
                if !TextOutW(dc, TEXT_X, 100, &label.encode_utf16().collect::<Vec<_>>()).as_bool() {
                    return Err(E_FAIL.into());
                }
                Ok((line_y as usize, spans))
            })();
            let restored = RestoreDC(dc, saved).as_bool();
            let (line_y, spans) = rendered?;
            if !restored || !GdiFlush().as_bool() {
                return Err(E_FAIL.into());
            }
            let pixels = std::slice::from_raw_parts(bits.cast::<u8>(), WIDTH * HEIGHT * 4).to_vec();
            drop(surface);
            Ok(Self {
                pixels,
                line_y,
                spans,
            })
        }
    }
    pub(crate) fn ink(&self, range: Range<usize>) -> usize {
        range
            .filter(|x| {
                let index = (self.line_y * WIDTH + x) * 4;
                self.pixels[index..index + 3] != [255, 255, 255]
            })
            .count()
    }
    pub(crate) fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(path)?;
        file.write_all(b"BM")?;
        file.write_all(&((54 + self.pixels.len()) as u32).to_le_bytes())?;
        file.write_all(&[0; 4])?;
        file.write_all(&54u32.to_le_bytes())?;
        file.write_all(&40u32.to_le_bytes())?;
        file.write_all(&(WIDTH as i32).to_le_bytes())?;
        file.write_all(&(-(HEIGHT as i32)).to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&32u16.to_le_bytes())?;
        file.write_all(&[0; 24])?;
        file.write_all(&self.pixels)
    }
}
