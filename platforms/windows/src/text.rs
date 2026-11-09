//! Text through GDI: render white-on-black with greyscale antialiasing into a DIB and use the
//! brightness as coverage. Gives the system font, with GDI's font linking for scripts Segoe UI
//! lacks, at no dependency cost.

use crate::util::wide;
use prism_core::render::{TextMask, TextRaster};
use std::cell::RefCell;
use std::collections::HashMap;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
    DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CALCRECT, DT_EXPANDTABS, DT_LEFT, DT_NOPREFIX, DT_TOP, DeleteDC, DeleteObject,
    DrawTextW, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, GdiFlush, GetTextFaceW, HDC, HFONT, OUT_TT_PRECIS, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows::core::PCWSTR;

const FACE: &str = "Segoe UI";

/// Geist, the interface typeface, built into the exe. It's scaled up 3% so it sits as large as
/// Inter did. See `assets/fonts/Geist-LICENSE.txt`.
const GEIST: &[u8] = include_bytes!("../../../assets/fonts/GeistVariable.ttf");
const GEIST_FACE: &str = "Geist";

/// Makes Geist available to this process only: nothing is installed on the system.
fn load_geist() -> bool {
    use windows::Win32::Graphics::Gdi::AddFontMemResourceEx;
    let mut count = 0u32;
    let handle = unsafe { AddFontMemResourceEx(GEIST.as_ptr() as *const _, GEIST.len() as u32, None, &mut count) };
    !handle.is_invalid() && count > 0
}

/// The face GDI really gives for `wanted`: the same name if it has it, or its fallback.
fn resolves_to(wanted: &'static str) -> bool {
    let t = GdiText::with_face(wanted);
    let font = t.font(12, FW_NORMAL.0);
    let mut face = [0u16; 64];
    let len = unsafe {
        let old = SelectObject(t.dc, font.into());
        let len = GetTextFaceW(t.dc, Some(&mut face));
        SelectObject(t.dc, old);
        len
    };
    String::from_utf16_lossy(&face[..(len.max(1) - 1) as usize]).eq_ignore_ascii_case(wanted)
}

/// Prism's typeface: Geist, or if it couldn't be loaded, Windows 11's text face, or the older one.
pub fn ui_face() -> &'static str {
    static FOUND: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    FOUND.get_or_init(|| {
        const NEW: &str = "Segoe UI Variable Text";
        if load_geist() && resolves_to(GEIST_FACE) {
            GEIST_FACE
        } else if resolves_to(NEW) {
            NEW
        } else {
            FACE
        }
    })
}

pub struct GdiText {
    dc: HDC,
    face: &'static str,
    fonts: RefCell<HashMap<(u32, u32), HFONT>>,
    cache: RefCell<HashMap<(String, u32, u32), std::rc::Rc<TextMask>>>,
    /// Sizes already measured: a page of text is measured many times a frame.
    sizes: RefCell<HashMap<(String, u32, u32), (i32, i32)>>,
}

/// The weight `bold` has always meant here.
fn weight_of(bold: bool) -> u32 {
    if bold { FW_SEMIBOLD.0 } else { FW_NORMAL.0 }
}

impl GdiText {
    pub fn new() -> GdiText {
        GdiText::with_face(ui_face())
    }

    pub fn with_face(face: &'static str) -> GdiText {
        let dc = unsafe { CreateCompatibleDC(None) };
        GdiText { dc, face, fonts: RefCell::new(HashMap::new()), cache: RefCell::new(HashMap::new()), sizes: RefCell::new(HashMap::new()) }
    }

    fn font(&self, px: u32, weight: u32) -> HFONT {
        *self.fonts.borrow_mut().entry((px, weight)).or_insert_with(|| {
            let face = wide(self.face);
            unsafe {
                CreateFontW(
                    -(px as i32),
                    0,
                    0,
                    0,
                    weight as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_TT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    ANTIALIASED_QUALITY,
                    DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
                    PCWSTR(face.as_ptr()),
                )
            }
        })
    }

    /// Width and height of `text` in pixels. An empty string measures as one blank line.
    pub fn measure(&self, text: &str, px: f32, bold: bool) -> (i32, i32) {
        self.measure_weight(text, px, weight_of(bold))
    }

    /// `measure` at a weight from 100 to 900.
    pub fn measure_weight(&self, text: &str, px: f32, weight: u32) -> (i32, i32) {
        let px = px.round().max(6.0) as u32;
        let key = (text.to_string(), px, weight);
        if let Some(size) = self.sizes.borrow().get(&key) {
            return *size;
        }
        let size = self.measure_uncached(text, px, weight);
        let mut sizes = self.sizes.borrow_mut();
        if sizes.len() > 4096 {
            sizes.clear();
        }
        sizes.insert(key, size);
        size
    }

    fn measure_uncached(&self, text: &str, px: u32, weight: u32) -> (i32, i32) {
        let t = if text.is_empty() { " ".to_string() } else { text.to_string() };
        // A trailing newline still adds a line: measure it with a space after.
        let t = if t.ends_with('\n') { format!("{t} ") } else { t };
        let mut w: Vec<u16> = t.encode_utf16().collect();
        let mut r = RECT::default();
        unsafe {
            let old = SelectObject(self.dc, self.font(px, weight).into());
            DrawTextW(self.dc, &mut w, &mut r, DT_CALCRECT | DT_NOPREFIX | DT_EXPANDTABS | DT_LEFT | DT_TOP);
            SelectObject(self.dc, old);
        }
        if text.is_empty() { (0, r.bottom - r.top) } else { (r.right - r.left, r.bottom - r.top) }
    }

    fn render(&self, text: &str, px: u32, weight: u32) -> Option<TextMask> {
        let (w, h) = self.measure_weight(text, px as f32, weight);
        if w <= 0 || h <= 0 {
            return None;
        }
        // A little slack: italic overhangs and antialiasing can spill a pixel past the box.
        let (bw, bh) = (w + 4, h + 2);
        unsafe {
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: bw,
                    biHeight: -bh,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let dib = CreateDIBSection(Some(self.dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let old_bmp = SelectObject(self.dc, dib.into());
            let old_font = SelectObject(self.dc, self.font(px, weight).into());
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(self.dc, COLORREF(0x00FF_FFFF));
            let mut r = RECT { left: 1, top: 0, right: bw, bottom: bh };
            let mut s: Vec<u16> = text.encode_utf16().collect();
            DrawTextW(self.dc, &mut s, &mut r, DT_NOPREFIX | DT_EXPANDTABS | DT_LEFT | DT_TOP);
            let _ = GdiFlush();
            let px_data = std::slice::from_raw_parts(bits as *const u8, (bw * bh * 4) as usize);
            let data = px_data.chunks_exact(4).map(|p| p[0].max(p[1]).max(p[2])).collect();
            SelectObject(self.dc, old_font);
            SelectObject(self.dc, old_bmp);
            let _ = DeleteObject(dib.into());
            Some(TextMask { w: bw as u32, h: bh as u32, data })
        }
    }

    /// Cached render, for labels redrawn every frame.
    pub fn mask(&self, text: &str, px: f32, bold: bool) -> Option<std::rc::Rc<TextMask>> {
        self.mask_weight(text, px, weight_of(bold))
    }

    /// `mask` at a weight from 100 to 900.
    pub fn mask_weight(&self, text: &str, px: f32, weight: u32) -> Option<std::rc::Rc<TextMask>> {
        let px = px.round().max(6.0) as u32;
        let key = (text.to_string(), px, weight);
        if let Some(m) = self.cache.borrow().get(&key) {
            return Some(m.clone());
        }
        let m = std::rc::Rc::new(self.render(text, px, weight)?);
        let mut cache = self.cache.borrow_mut();
        if cache.len() > 1024 {
            cache.clear();
        }
        cache.insert(key, m.clone());
        Some(m)
    }
}

impl TextRaster for GdiText {
    fn raster(&self, text: &str, px: f32, bold: bool) -> Option<TextMask> {
        self.mask(text, px, bold).map(|m| TextMask { w: m.w, h: m.h, data: m.data.clone() })
    }
}

impl Drop for GdiText {
    fn drop(&mut self) {
        unsafe {
            for (_, f) in self.fonts.borrow_mut().drain() {
                let _ = DeleteObject(f.into());
            }
            let _ = DeleteDC(self.dc);
        }
    }
}
