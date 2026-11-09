//! Grabbing the screen, and the window and monitor rectangles the overlay snaps to.
//!
//! Capture is one `BitBlt` of the whole virtual desktop into a 32-bit top-down DIB, which GDI
//! routes through DWM's composed frame. That gets layered windows (with `CAPTUREBLT`) and every
//! monitor in one call, in a few milliseconds.

use prism_core::{IRect, Image};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, EnumDisplayMonitors, GetDC, HDC, HMONITOR, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, EnumWindows, GWL_EXSTYLE, GetClassNameW, GetCursorInfo, GetIconInfo,
    GetSystemMetrics, GetWindowLongW, GetWindowTextLengthW, ICONINFO, IsIconic, IsWindowVisible, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};
use windows::core::BOOL;

/// The virtual desktop: all monitors, in screen coordinates.
pub fn virtual_screen() -> IRect {
    unsafe {
        IRect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// A small grab for the live magnifier, taken on every mouse move. It leaves out `CAPTUREBLT`,
/// which can make the pointer flicker when used that often.
pub fn grab_quick(area: IRect) -> Option<Image> {
    grab_with(area, false, false)
}

/// Copy `area` (screen coordinates) into a BGRA image. Pixels are made opaque.
pub fn grab(area: IRect, with_cursor: bool) -> Option<Image> {
    grab_with(area, with_cursor, true)
}

fn grab_with(area: IRect, with_cursor: bool, layered: bool) -> Option<Image> {
    if area.is_empty() {
        return None;
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: area.w,
                biHeight: -area.h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            return None;
        };
        let old = SelectObject(mem, dib.into());
        let ok = BitBlt(mem, 0, 0, area.w, area.h, Some(screen), area.x, area.y, if layered { SRCCOPY | CAPTUREBLT } else { SRCCOPY }).is_ok();
        if ok && with_cursor {
            draw_cursor(mem, area.x, area.y);
        }
        let len = area.w as usize * area.h as usize * 4;
        let mut data = Vec::with_capacity(len);
        if ok {
            data.extend_from_slice(std::slice::from_raw_parts(bits as *const u8, len));
        }
        SelectObject(mem, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if !ok {
            return None;
        }
        for p in data.chunks_exact_mut(4) {
            p[3] = 255;
        }
        Image::from_bgra(area.w as u32, area.h as u32, data)
    }
}

unsafe fn draw_cursor(dc: HDC, ox: i32, oy: i32) {
    unsafe {
        let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        if GetCursorInfo(&mut ci).is_err() || ci.flags != CURSOR_SHOWING {
            return;
        }
        let mut ii = ICONINFO::default();
        if GetIconInfo(ci.hCursor.into(), &mut ii).is_err() {
            return;
        }
        let (hx, hy) = (ii.xHotspot as i32, ii.yHotspot as i32);
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(ii.hbmMask.into());
        }
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(ii.hbmColor.into());
        }
        let _ = DrawIconEx(dc, ci.ptScreenPos.x - ox - hx, ci.ptScreenPos.y - oy - hy, ci.hCursor.into(), 0, 0, 0, None, DI_NORMAL);
    }
}

/// A window the overlay can snap to, in image coordinates (relative to the virtual desktop).
#[derive(Clone, Debug)]
pub struct WinRect {
    pub rect: IRect,
}

struct Collect {
    skip: HWND,
    origin: (i32, i32),
    out: Vec<WinRect>,
}

/// Visible top-level windows, front to back, using DWM's frame bounds (without the invisible
/// resize borders Windows 10 and 11 add around windows).
pub fn windows_front_to_back(skip: HWND, origin: (i32, i32)) -> Vec<WinRect> {
    let mut c = Collect { skip, origin, out: Vec::with_capacity(64) };
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut c as *mut Collect as isize));
    }
    c.out
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lp: LPARAM) -> BOOL {
    unsafe {
        let c = &mut *(lp.0 as *mut Collect);
        if hwnd == c.skip || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return TRUE;
        }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TRANSPARENT.0 != 0 {
            return TRUE;
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4);
        if cloaked != 0 {
            return TRUE;
        }
        // Untitled tool windows are mostly invisible helpers (tooltips, shadows, IME bars).
        if ex & WS_EX_TOOLWINDOW.0 != 0 && GetWindowTextLengthW(hwnd) == 0 {
            return TRUE;
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        // The desktop itself: the monitor is the better snap target there.
        if class == "Progman" || class == "WorkerW" {
            return TRUE;
        }
        let mut r = RECT::default();
        if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32).is_err() {
            return TRUE;
        }
        let rect = IRect::from_ltrb(r.left - c.origin.0, r.top - c.origin.1, r.right - c.origin.0, r.bottom - c.origin.1);
        if rect.w >= 8 && rect.h >= 8 {
            c.out.push(WinRect { rect });
        }
        TRUE
    }
}

/// Monitor rectangles in image coordinates.
pub fn monitors(origin: (i32, i32)) -> Vec<IRect> {
    let mut out: Vec<IRect> = Vec::new();
    unsafe extern "system" fn proc_(_: HMONITOR, _: HDC, r: *mut RECT, lp: LPARAM) -> BOOL {
        unsafe {
            let out = &mut *(lp.0 as *mut Vec<IRect>);
            let r = &*r;
            out.push(IRect::from_ltrb(r.left, r.top, r.right, r.bottom));
        }
        TRUE
    }
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(proc_), LPARAM(&mut out as *mut _ as isize));
    }
    out.into_iter().map(|r| r.offset(-origin.0, -origin.1)).collect()
}
