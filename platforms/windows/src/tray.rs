//! Notification-area icon and its menu. Event driven: no timers.

use crate::util::{APP_NAME, wide};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetSystemMetrics, HICON, HMENU, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW,
    MF_SEPARATOR, MF_STRING, SM_CXSMICON, SM_CYSMICON, SetForegroundWindow, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
};
use windows::core::PCWSTR;

const TRAY_ID: u32 = 1;

fn copy_truncated(dst: &mut [u16], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(dst.len() - 1).collect();
    dst[..units.len()].copy_from_slice(&units);
    dst[units.len()] = 0;
}

pub fn app_icon(cx: i32, cy: i32) -> HICON {
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        LoadImageW(Some(module.into()), PCWSTR(1 as *const u16), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR).map(|h| HICON(h.0)).unwrap_or_default()
    }
}

fn data(hwnd: HWND, message: u32) -> NOTIFYICONDATAW {
    let mut d = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: message,
        ..Default::default()
    };
    d.hIcon = app_icon(unsafe { GetSystemMetrics(SM_CXSMICON) }, unsafe { GetSystemMetrics(SM_CYSMICON) });
    d
}

pub fn add(hwnd: HWND, message: u32, tip: &str) {
    let mut d = data(hwnd, message);
    copy_truncated(&mut d.szTip, tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &d);
    }
}

pub fn set_tip(hwnd: HWND, message: u32, tip: &str) {
    let mut d = data(hwnd, message);
    copy_truncated(&mut d.szTip, tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
    }
}

pub fn remove(hwnd: HWND, message: u32) {
    let d = data(hwnd, message);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &d);
    }
}

/// What the tray menu offers. Everything else is in Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Region = 1,
    Screen,
    Text,
    OpenFolder,
    Settings,
    Quit,
}

const ALL: [Choice; 6] = [Choice::Region, Choice::Screen, Choice::Text, Choice::OpenFolder, Choice::Settings, Choice::Quit];

fn add_item(menu: HMENU, c: Choice, text: &str) {
    let label = wide(text);
    unsafe {
        let _ = AppendMenuW(menu, MF_STRING, c as usize, PCWSTR(label.as_ptr()));
    }
}

fn sep(menu: HMENU) {
    unsafe {
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    }
}

fn with_key(label: &str, key: &str) -> String {
    if key.is_empty() { label.to_string() } else { format!("{label}\t{key}") }
}

/// Run the menu. Modal: the caller must not hold app state across this call. `hotkeys`: what starts a
/// region, screen and text capture, shown beside them.
pub fn show_menu(hwnd: HWND, hotkeys: &[String; 3]) -> Option<Choice> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        add_item(menu, Choice::Region, &with_key("Capture region", &hotkeys[0]));
        add_item(menu, Choice::Screen, &with_key("Capture screen", &hotkeys[1]));
        add_item(menu, Choice::Text, &with_key("Copy text from screen", &hotkeys[2]));
        sep(menu);
        add_item(menu, Choice::OpenFolder, "Open screenshots folder");
        add_item(menu, Choice::Settings, "Settings\u{2026}");
        sep(menu);
        add_item(menu, Choice::Quit, &format!("Quit {APP_NAME}"));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
        ALL.into_iter().find(|c| *c as i32 == chosen.0)
    }
}
