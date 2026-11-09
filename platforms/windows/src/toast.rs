//! A small card in the corner after copying or saving: thumbnail, what happened, and a click
//! target. It's our own layered window rather than a Windows notification, so it appears
//! instantly, never piles up in the notification centre and goes away by itself.

use crate::text::GdiText;
use crate::ui;
use crate::util::wide;
use prism_core::{IRect, Image, Rgba};
use prism_core::settings::PopupPosition;
use std::cell::RefCell;
use std::path::PathBuf;
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, GetMonitorInfoW, HBITMAP, HDC, HGDIOBJ, MONITORINFO, ReleaseDC, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, IDC_ARROW, IDC_HAND, KillTimer, LoadCursorW, SetCursor, WM_SETCURSOR, RegisterClassExW, SW_SHOWNOACTIVATE, SetTimer,
    ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::PCWSTR;

#[derive(Clone)]
pub enum Click {
    None,
    /// Show the file in Explorer.
    Reveal(PathBuf),
    /// Open with the default app.
    Open(PathBuf),
    /// Install the downloaded update.
    Install,
    /// Just close.
    Dismiss,
}

struct Live {
    hwnd: HWND,
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    pos: POINT,
    size: SIZE,
    alpha: u8,
    fading: bool,
    click: Click,
    /// Where a click does something, in the window. Empty: anywhere does `click`.
    buttons: Vec<(IRect, Click)>,
    /// How long it stays when not pointed at.
    life: u32,
}

impl Drop for Live {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

thread_local! {
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
    static POSITION: std::cell::Cell<PopupPosition> = const { std::cell::Cell::new(PopupPosition::BottomRight) };
}

const CLASS: &str = "PrismToast";
const LIFE_TIMER: usize = 1;
const FADE_TIMER: usize = 2;
const LIFE_MS: u32 = 3200;

fn register() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let name = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
            lpszClassName: PCWSTR(name.as_ptr()),
            hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        std::mem::forget(name);
    });
}

/// Average `factor`×`factor` blocks: a clean first step when shrinking a lot.
fn box_down(img: &Image, factor: u32) -> Image {
    if factor <= 1 {
        return img.clone();
    }
    let (w, h) = ((img.w / factor).max(1), (img.h / factor).max(1));
    let mut out = Image::new(w, h);
    let n = factor * factor;
    for y in 0..h {
        for x in 0..w {
            let mut s = [0u32; 4];
            for dy in 0..factor {
                let row = ((y * factor + dy).min(img.h - 1) * img.w) as usize;
                for dx in 0..factor {
                    let i = (row + (x * factor + dx).min(img.w - 1) as usize) * 4;
                    for c in 0..4 {
                        s[c] += img.data[i + c] as u32;
                    }
                }
            }
            let o = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                out.data[o + c] = (s[c] / n) as u8;
            }
        }
    }
    out
}

/// A thumbnail that fits in `max_w`×`max_h`.
pub fn thumbnail(img: &Image, max_w: u32, max_h: u32) -> Image {
    if img.w == 0 || img.h == 0 {
        return Image::new(1, 1);
    }
    let k = (max_w as f32 / img.w as f32).min(max_h as f32 / img.h as f32).min(1.0);
    let factor = ((1.0 / k) / 2.0).floor().max(1.0) as u32;
    let pre = box_down(img, factor);
    let (tw, th) = (((img.w as f32 * k).round() as u32).max(1), ((img.h as f32 * k).round() as u32).max(1));
    let Some(src) = Pixmap::from_vec(pre.data.clone(), tiny_skia::IntSize::from_wh(pre.w, pre.h).unwrap()) else { return pre };
    let Some(mut dst) = Pixmap::new(tw, th) else { return pre };
    let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
    dst.draw_pixmap(0, 0, src.as_ref(), &paint, Transform::from_scale(tw as f32 / pre.w as f32, th as f32 / pre.h as f32), None);
    Image::from_bgra(tw, th, dst.take()).unwrap_or(pre)
}

fn ellipsize(t: &GdiText, s: &str, px: f32, bold: bool, max_w: i32) -> String {
    let first_line = s.lines().next().unwrap_or("");
    let mut out = first_line.to_string();
    if t.measure(&out, px, bold).0 <= max_w && first_line.len() == s.trim_end().len() {
        return out;
    }
    loop {
        let candidate = format!("{}…", out.trim_end());
        if t.measure(&candidate, px, bold).0 <= max_w || out.is_empty() {
            return candidate;
        }
        out.pop();
    }
}

/// Where toasts appear from now on.
pub fn set_position(position: PopupPosition) {
    POSITION.with(|p| p.set(position));
}

/// Where a toast of `size` goes in `work`, the part of a monitor not under the taskbar, keeping
/// `inset` from the edges. `size` includes a `margin` of shadow on each side.
fn place(position: PopupPosition, work: IRect, size: (i32, i32), margin: i32, inset: i32) -> (i32, i32) {
    use PopupPosition::*;
    let (w, h) = size;
    let x = match position {
        BottomLeft | TopLeft => work.x - margin + inset,
        BottomCenter | TopCenter => work.x + (work.w - w) / 2,
        BottomRight | TopRight => work.right() - w + margin - inset,
    };
    let y = match position {
        TopLeft | TopCenter | TopRight => work.y - margin + inset,
        _ => work.bottom() - h + margin - inset,
    };
    (x, y)
}

/// Show a toast in the chosen corner of the monitor containing `near` (screen coordinates), or
/// of the cursor's monitor.
pub fn show(title: &str, body: &str, thumb: Option<&Image>, near: Option<IRect>, click: Click) {
    close();
    register();
    let anchor = near.map(|r| POINT { x: r.x + r.w / 2, y: r.y + r.h / 2 }).unwrap_or_else(crate::util::cursor_pos);
    let mon = crate::util::monitor_at(anchor);
    let s = crate::util::monitor_scale(mon);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let _ = GetMonitorInfoW(mon, &mut mi);
    }
    let work = mi.rcWork;
    let text = GdiText::new();

    let margin = (14.0 * s) as i32; // room for the shadow
    let pad = (12.0 * s) as i32;
    let thumb_box = ((96.0 * s) as u32, (58.0 * s) as u32);
    let card_w = (340.0 * s) as i32;
    let card_h = (thumb_box.1 as i32 + pad * 2).max((60.0 * s) as i32);
    let (w, h) = (card_w + margin * 2, card_h + margin * 2);
    let Some(mut pm) = Pixmap::new(w as u32, h as u32) else { return };
    let card = IRect::new(margin, margin, card_w, card_h);
    {
        let mut m = pm.as_mut();
        ui::panel(&mut m, card, 10.0 * s, s);
    }
    let mut text_x = card.x + pad;
    if let Some(img) = thumb {
        let t = thumbnail(img, thumb_box.0, thumb_box.1);
        let tx = card.x + pad + (thumb_box.0 as i32 - t.w as i32) / 2;
        let ty = card.y + (card_h - t.h as i32) / 2;
        if let Some(src) = Pixmap::from_vec(t.data.clone(), tiny_skia::IntSize::from_wh(t.w, t.h).unwrap()) {
            pm.draw_pixmap(tx, ty, src.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        let mut m = pm.as_mut();
        ui::stroke_rrect(&mut m, IRect::new(tx, ty, t.w as i32, t.h as i32), 0.0, 1.0, ui::PANEL_BORDER);
        text_x = card.x + pad + thumb_box.0 as i32 + pad;
    }
    let max_text = card.right() - pad - text_x;
    let (title_px, body_px) = (13.5 * s, 12.0 * s);
    let title = ellipsize(&text, title, title_px, true, max_text);
    let body = ellipsize(&text, body, body_px, false, max_text);
    let (_, th) = text.measure(&title, title_px, true);
    let (_, bh) = text.measure(&body, body_px, false);
    let gap = (2.0 * s) as i32;
    let block = th + if body.is_empty() { 0 } else { gap + bh };
    let ty = card.y + (card_h - block) / 2;
    {
        let mut m = pm.as_mut();
        ui::text(&mut m, &text, &title, title_px, true, text_x, ty, ui::FG);
        if !body.is_empty() {
            ui::text(&mut m, &text, &body, body_px, false, text_x, ty + th + gap, ui::FG_DIM);
        }
    }
    // tiny-skia drew in "BGRA via swapped colours", so the buffer is already what Windows wants.
    let work = IRect::new(work.left, work.top, work.right - work.left, work.bottom - work.top);
    let (x, y) = place(POSITION.with(|p| p.get()), work, (w, h), margin, (16.0 * s) as i32);
    open_window(&pm, POINT { x, y }, (w, h), click, Vec::new(), LIFE_MS);
}

/// How long the offer to install an update stays when not pointed at.
const OFFER_MS: u32 = 20_000;

/// Offers to install the version just downloaded: Prism's icon, what's ready, and "Install
/// now" and "Later". If it goes unanswered, the tray menu and Settings still have it.
pub fn show_update(version: &str) {
    close();
    register();
    let mon = crate::util::monitor_at(crate::util::cursor_pos());
    let s = crate::util::monitor_scale(mon);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let _ = GetMonitorInfoW(mon, &mut mi);
    }
    let work = IRect::new(mi.rcWork.left, mi.rcWork.top, mi.rcWork.right - mi.rcWork.left, mi.rcWork.bottom - mi.rcWork.top);
    let Some((pm, margin, buttons)) = draw_update(version, s) else { return };
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let (x, y) = place(POSITION.with(|p| p.get()), work, (w, h), margin, (16.0 * s).round() as i32);
    open_window(&pm, POINT { x, y }, (w, h), Click::None, buttons, OFFER_MS);
}

/// The update offer at `s` times its size, with its shadow margin and its buttons.
fn draw_update(version: &str, s: f32) -> Option<(Pixmap, i32, Vec<(IRect, Click)>)> {
    let text = GdiText::new();
    let px = |v: f32| (v * s).round() as i32;
    let (margin, pad, icon) = (px(14.0), px(14.0), px(40.0));
    let card_w = px(340.0);
    let (title_px, body_px, button_px) = (13.5 * s, 12.0 * s, 12.5 * s);
    let title = format!("{} {version} is ready", crate::util::APP_NAME);
    let body = "Install it now? It takes a few seconds.";
    let (_, th) = text.measure(&title, title_px, true);
    let (_, bh) = text.measure(body, body_px, false);
    let gap = px(3.0);
    let button_h = px(28.0);
    let top_h = icon.max(th + gap + bh);
    let card_h = pad + top_h + px(12.0) + button_h + pad;
    let (w, h) = (card_w + margin * 2, card_h + margin * 2);
    let mut pm = Pixmap::new(w as u32, h as u32)?;
    let card = IRect::new(margin, margin, card_w, card_h);
    let later_w = text.measure("Later", button_px, true).0 + px(28.0);
    let install_w = text.measure("Install now", button_px, true).0 + px(32.0);
    let by = card.bottom() - pad - button_h;
    let install = IRect::new(card.right() - pad - install_w, by, install_w, button_h);
    let later = IRect::new(install.x - px(8.0) - later_w, by, later_w, button_h);
    {
        let mut m = pm.as_mut();
        ui::panel(&mut m, card, 10.0 * s, s);
        let text_x = card.x + pad + icon + px(12.0);
        let ty = card.y + pad + (top_h - (th + gap + bh)) / 2;
        ui::text(&mut m, &text, &title, title_px, true, text_x, ty, ui::FG);
        ui::text(&mut m, &text, body, body_px, false, text_x, ty + th + gap, ui::FG_DIM);
        let radius = button_h as f32 / 2.0;
        ui::stroke_rrect(&mut m, later, radius, 1.0, ui::PANEL_BORDER);
        ui::fill_rrect(&mut m, later, radius, Rgba { r: 0xFF, g: 0xFF, b: 0xFF, a: 0x10 });
        ui::fill_rrect(&mut m, install, radius, Rgba::rgb(0x7C, 0x3A, 0xED));
        for (r, label) in [(later, "Later"), (install, "Install now")] {
            let (lw, lh) = text.measure(label, button_px, true);
            ui::text(&mut m, &text, label, button_px, true, r.x + (r.w - lw) / 2, r.y + (r.h - lh) / 2, ui::FG);
        }
    }
    // The icon is drawn RGBA; this canvas is BGRA.
    let mut logo = prism_core::icon::render_rgba(icon as u32);
    logo.data_mut().chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
    pm.draw_pixmap(card.x + pad, card.y + pad + (top_h - icon) / 2, logo.as_ref(), &PixmapPaint::default(), Transform::identity(), None);

    // Generous edges, so a click just off a button still counts.
    let buttons = vec![(install.inflate(px(3.0)), Click::Install), (later.inflate(px(3.0)), Click::Dismiss)];
    Some((pm, margin, buttons))
}

/// The update offer as a PNG, on a backdrop, for checking how it looks.
pub fn preview_update(out: &std::path::Path, scale: f32) -> i32 {
    let Some((popup, _, _)) = draw_update("1.1.0", scale) else { return 3 };
    let Some(mut pm) = Pixmap::new(popup.width() + 40, popup.height() + 40) else { return 3 };
    pm.fill(tiny_skia::Color::from_rgba8(0x60, 0x60, 0x66, 0xFF));
    pm.draw_pixmap(20, 20, popup.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    let Some(image) = Image::from_bgra(pm.width(), pm.height(), pm.take()) else { return 6 };
    match prism_core::image::encode_png(&image).map(|png| std::fs::write(out, png)) {
        Ok(Ok(())) => 0,
        _ => 7,
    }
}

/// Puts a drawn toast on the screen. `buttons` are where a click does something, in the window;
/// with none, a click anywhere does `click`.
fn open_window(pm: &Pixmap, pos: POINT, (w, h): (i32, i32), click: Click, buttons: Vec<(IRect, Click)>, life: u32) {
    unsafe {
        let class = wide(CLASS);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            pos.x,
            pos.y,
            w,
            h,
            None,
            None,
            Some(GetModuleHandleW(None).unwrap_or_default().into()),
            None,
        ) else {
            return;
        };
        let dc = CreateCompatibleDC(None);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            let _ = DestroyWindow(hwnd);
            return;
        };
        std::ptr::copy_nonoverlapping(pm.data().as_ptr(), bits as *mut u8, pm.data().len());
        let old = SelectObject(dc, bmp.into());
        let live = Live { hwnd, dc, bmp, old, pos, size: SIZE { cx: w, cy: h }, alpha: 255, fading: false, click, buttons, life };
        present(&live);
        LIVE.with(|l| *l.borrow_mut() = Some(live));
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), LIFE_TIMER, life, None);
    }
}

/// What a click at `at`, in the window, does: with buttons, only on one of them.
fn clicked(l: &Live, (x, y): (i32, i32)) -> Option<Click> {
    if l.buttons.is_empty() {
        return Some(l.click.clone());
    }
    l.buttons.iter().find(|(r, _)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom()).map(|(_, c)| c.clone())
}

fn present(l: &Live) {
    unsafe {
        let screen = GetDC(None);
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: l.alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
        let _ = UpdateLayeredWindow(l.hwnd, Some(screen), Some(&l.pos), Some(&l.size), Some(l.dc), Some(&src), Default::default(), Some(&blend), ULW_ALPHA);
        ReleaseDC(None, screen);
    }
}

pub fn close() {
    let live = LIVE.with(|l| l.borrow_mut().take());
    if let Some(l) = live {
        unsafe {
            let _ = DestroyWindow(l.hwnd);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER if wp.0 == LIFE_TIMER => {
                let _ = KillTimer(Some(hwnd), LIFE_TIMER);
                LIVE.with(|l| {
                    if let Some(l) = l.borrow_mut().as_mut() {
                        l.fading = true;
                    }
                });
                SetTimer(Some(hwnd), FADE_TIMER, 16, None);
                LRESULT(0)
            }
            WM_TIMER if wp.0 == FADE_TIMER => {
                let done = LIVE.with(|l| {
                    let mut b = l.borrow_mut();
                    let Some(l) = b.as_mut() else { return true };
                    l.alpha = l.alpha.saturating_sub(24);
                    present(l);
                    l.alpha == 0
                });
                if done {
                    close();
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                // Hovering keeps it up, and cancels a fade in progress.
                LIVE.with(|l| {
                    if let Some(l) = l.borrow_mut().as_mut() {
                        if l.fading {
                            l.fading = false;
                            l.alpha = 255;
                            let _ = KillTimer(Some(hwnd), FADE_TIMER);
                            present(l);
                        }
                    }
                });
                let life = LIVE.with(|l| l.borrow().as_ref().map_or(LIFE_MS, |l| l.life));
                SetTimer(Some(hwnd), LIFE_TIMER, life, None);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let at = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
                let Some(click) = LIVE.with(|l| l.borrow().as_ref().and_then(|l| clicked(l, at))) else { return LRESULT(0) };
                close();
                match Some(click) {
                    Some(Click::Reveal(p)) => crate::util::reveal_in_explorer(&p),
                    Some(Click::Open(p)) => crate::util::shell_open(&p),
                    Some(Click::Install) => crate::update::install(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                // A hand only where a click does something.
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                let over = LIVE.with(|l| l.borrow().as_ref().map(|l| clicked(l, (p.x - l.pos.x, p.y - l.pos.y)).is_some()));
                let cursor = if over == Some(false) { IDC_ARROW } else { IDC_HAND };
                SetCursor(LoadCursorW(None, cursor).ok());
                LRESULT(1)
            }
            WM_RBUTTONUP => {
                close();
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popups_go_where_asked_inside_the_work_area() {
        let work = IRect::new(0, 0, 1000, 800);
        let (size, margin, inset) = ((300, 100), 10, 16);
        assert_eq!(place(PopupPosition::BottomRight, work, size, margin, inset), (1000 - 300 + 10 - 16, 800 - 100 + 10 - 16));
        assert_eq!(place(PopupPosition::TopLeft, work, size, margin, inset), (6, 6));
        assert_eq!(place(PopupPosition::TopCenter, work, size, margin, inset), (350, 6));
        assert_eq!(place(PopupPosition::BottomLeft, IRect::new(-1920, 0, 1920, 1040), size, margin, inset).0, -1920 + 6);
    }

    #[test]
    fn thumbnail_fits_and_keeps_aspect() {
        let img = Image::new(2560, 1440);
        let t = thumbnail(&img, 96, 58);
        assert!(t.w <= 96 && t.h <= 58);
        assert!((t.w as f32 / t.h as f32 - 2560.0 / 1440.0).abs() < 0.1);
        let small = Image::new(20, 10);
        let t = thumbnail(&small, 96, 58);
        assert_eq!((t.w, t.h), (20, 10));
    }
}
