//! Putting images and text on the clipboard.
//!
//! Images go on as both `CF_DIB` (every Windows app reads it; Windows derives `CF_BITMAP` and
//! `CF_DIBV5` from it) and the registered "PNG" format, which browsers, chat apps and Office
//! prefer because it's lossless and exact.

use crate::util::wide;
use prism_core::Image;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows::core::PCWSTR;

fn global_from(bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).ok()?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

/// A packed bottom-up 32-bit DIB: header then pixels. Bottom-up is what older readers expect.
fn dib_bytes(img: &Image) -> Vec<u8> {
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: img.w as i32,
        biHeight: img.h as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: img.w * img.h * 4,
        ..Default::default()
    };
    let mut out = Vec::with_capacity(std::mem::size_of::<BITMAPINFOHEADER>() + img.data.len());
    out.extend_from_slice(unsafe {
        std::slice::from_raw_parts(&header as *const _ as *const u8, std::mem::size_of::<BITMAPINFOHEADER>())
    });
    let stride = img.stride();
    for row in img.data.chunks_exact(stride).rev() {
        out.extend_from_slice(row);
    }
    out
}

/// Open the clipboard, retrying briefly: another app (often a clipboard manager reacting to the
/// last change) may hold it for a moment.
fn open(hwnd: HWND) -> bool {
    for attempt in 0..10 {
        if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10 + attempt * 5));
    }
    false
}

fn set(format: u32, bytes: &[u8]) -> bool {
    let Some(h) = global_from(bytes) else { return false };
    if unsafe { SetClipboardData(format, Some(HANDLE(h.0))) }.is_err() {
        // Ownership only passes to the clipboard on success.
        unsafe {
            let _ = GlobalFree(Some(h));
        }
        return false;
    }
    true
}

/// Put an image on the clipboard. `png` is optional so callers can skip encoding when it
/// failed; the DIB alone still works everywhere.
pub fn set_image(hwnd: HWND, img: &Image, png: Option<&[u8]>) -> bool {
    let dib = dib_bytes(img);
    if !open(hwnd) {
        return false;
    }
    let ok = unsafe {
        let _ = EmptyClipboard();
        let ok = set(CF_DIB.0 as u32, &dib);
        if let Some(png) = png {
            let name = wide("PNG");
            let fmt = RegisterClipboardFormatW(PCWSTR(name.as_ptr()));
            if fmt != 0 {
                set(fmt, png);
            }
        }
        ok
    };
    unsafe {
        let _ = CloseClipboard();
    }
    ok
}

pub fn set_text(hwnd: HWND, text: &str) -> bool {
    // Windows line endings, so Notepad and friends keep the lines.
    let normalized = text.replace("\r\n", "\n").replace('\n', "\r\n");
    let units: Vec<u16> = normalized.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(units.as_ptr() as *const u8, units.len() * 2) };
    if !open(hwnd) {
        return false;
    }
    let ok = unsafe {
        let _ = EmptyClipboard();
        set(CF_UNICODETEXT.0 as u32, bytes)
    };
    unsafe {
        let _ = CloseClipboard();
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_is_bottom_up_with_header() {
        let mut img = Image::new(1, 2);
        img.data.copy_from_slice(&[1, 1, 1, 255, 2, 2, 2, 255]);
        let d = dib_bytes(&img);
        let hs = std::mem::size_of::<BITMAPINFOHEADER>();
        assert_eq!(d.len(), hs + 8);
        assert_eq!(&d[hs..hs + 4], &[2, 2, 2, 255]);
        assert_eq!(&d[hs + 4..], &[1, 1, 1, 255]);
    }
}
