//! Small Win32 helpers shared across the host.

use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, GetMonitorInfoW};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::{FOLDERID_Pictures, FOLDERID_RoamingAppData, FOLDERID_Screenshots, KF_FLAG_CREATE, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, PostMessageW};
use windows::core::GUID;

pub const APP_NAME: &str = "Prism";

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// A `HWND` that can cross threads. Posting messages to it is safe from any thread.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SendHwnd(pub isize);
unsafe impl Send for SendHwnd {}
unsafe impl Sync for SendHwnd {}

impl SendHwnd {
    pub fn new(hwnd: HWND) -> Self {
        SendHwnd(hwnd.0 as isize)
    }
    pub fn get(self) -> HWND {
        HWND(self.0 as *mut _)
    }
    /// Post a boxed value; the receiver takes it back with [`take_boxed`].
    pub fn post_boxed<T: Send + 'static>(self, message: u32, wparam: usize, value: T) {
        let ptr = Box::into_raw(Box::new(value));
        let ok = unsafe { PostMessageW(Some(self.get()), message, WPARAM(wparam), LPARAM(ptr as isize)) };
        if ok.is_err() {
            // The window is gone; free the value instead of leaking it.
            drop(unsafe { Box::from_raw(ptr) });
        }
    }
}

/// Take back a value sent with [`SendHwnd::post_boxed`].
///
/// # Safety
/// `lparam` must come from `post_boxed` with the same `T`, and be taken exactly once.
pub unsafe fn take_boxed<T>(lparam: LPARAM) -> T {
    *unsafe { Box::from_raw(lparam.0 as *mut T) }
}

fn known_folder(id: &GUID, create: bool) -> Option<PathBuf> {
    unsafe {
        let flags = if create { KF_FLAG_CREATE } else { KF_FLAG_DEFAULT };
        let p = SHGetKnownFolderPath(id, flags, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// The app's name before it became Prism; its settings are carried over once.
const OLD_APP_NAME: &str = "BLS Screenshot";

/// Whether this copy is the portable one: its file name says so, as in `Prism-1.0.0-Portable.exe`.
pub fn is_portable() -> bool {
    std::env::current_exe().ok().and_then(|exe| exe.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase())).is_some_and(|stem| stem.contains("portable"))
}

/// Where settings live, created if needed. The portable copy keeps them in a "Prism settings"
/// folder beside itself, so it leaves nothing in the user's profile. Otherwise it's
/// `%APPDATA%\Prism`, and the first time that's created, settings from `%APPDATA%\BLS Screenshot`
/// are copied across (the old folder is left alone).
pub fn data_dir() -> PathBuf {
    if is_portable() {
        if let Some(beside) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|p| p.join(format!("{APP_NAME} settings")))) {
            let _ = std::fs::create_dir_all(&beside);
            return beside;
        }
    }
    let base = known_folder(&FOLDERID_RoamingAppData, false).unwrap_or_else(std::env::temp_dir);
    let dir = base.join(APP_NAME);
    if !dir.exists() {
        let old = base.join(OLD_APP_NAME).join("settings.json");
        let _ = std::fs::create_dir_all(&dir);
        if old.is_file() {
            let _ = std::fs::copy(&old, dir.join("settings.json"));
        }
    }
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The system Screenshots folder (Pictures\Screenshots), falling back to Pictures.
pub fn default_save_dir() -> PathBuf {
    known_folder(&FOLDERID_Screenshots, true)
        .or_else(|| known_folder(&FOLDERID_Pictures, false).map(|p| p.join("Screenshots")))
        .unwrap_or_else(|| std::env::temp_dir().join("Screenshots"))
}

pub fn cursor_pos() -> POINT {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    p
}

pub fn monitor_at(p: POINT) -> HMONITOR {
    unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST) }
}

/// Monitor rectangle in virtual-screen pixels: (left, top, right, bottom).
pub fn monitor_rect(m: HMONITOR) -> (i32, i32, i32, i32) {
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let _ = GetMonitorInfoW(m, &mut mi);
    }
    let r = mi.rcMonitor;
    (r.left, r.top, r.right, r.bottom)
}

/// Scale factor (1.0 at 96 DPI) of a monitor.
pub fn monitor_scale(m: HMONITOR) -> f32 {
    let (mut x, mut y) = (96u32, 96u32);
    unsafe {
        let _ = GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut x, &mut y);
    }
    (x as f32 / 96.0).clamp(1.0, 4.0)
}

/// Local time for file names.
pub fn now_stamp() -> prism_core::naming::Stamp {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    prism_core::naming::Stamp { year: t.wYear, month: t.wMonth as u8, day: t.wDay as u8, hour: t.wHour as u8, minute: t.wMinute as u8, second: t.wSecond as u8 }
}

/// Open a file or folder with its default handler.
pub fn shell_open(target: &Path) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let t = wide_path(target);
    let verb = wide("open");
    unsafe {
        ShellExecuteW(None, windows::core::PCWSTR(verb.as_ptr()), windows::core::PCWSTR(t.as_ptr()), None, None, SW_SHOWNORMAL);
    }
}

/// Show a file selected in Explorer.
pub fn reveal_in_explorer(file: &Path) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let args = wide(&format!("/select,\"{}\"", file.display()));
    let exe = wide("explorer.exe");
    unsafe {
        ShellExecuteW(None, windows::core::PCWSTR::null(), windows::core::PCWSTR(exe.as_ptr()), windows::core::PCWSTR(args.as_ptr()), None, SW_SHOWNORMAL);
    }
}

/// Write a debug line to `%APPDATA%\Prism\log.txt` when `PRISM_LOG` is set.
/// Off by default: the app writes nothing to disk unless asked.
pub fn log(msg: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("PRISM_LOG").is_some()) {
        return;
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(data_dir().join("log.txt")) {
        let _ = writeln!(f, "{msg}");
    }
}

/// Bring `hwnd` to the front with keyboard focus, even when Windows' foreground lock would
/// refuse. That happens when the request didn't come from input this process received, e.g.
/// Win+Shift+S caught by the keyboard hook. Attaching to the foreground thread's input queue for
/// a moment is the documented way to share its right to set the foreground.
pub fn force_foreground(hwnd: HWND) {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow};
    unsafe {
        if SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() == hwnd {
            let _ = SetFocus(Some(hwnd));
            return;
        }
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        let attached = fg_thread != 0 && fg_thread != me && windows::Win32::System::Threading::AttachThreadInput(me, fg_thread, true).as_bool();
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if attached {
            let _ = windows::Win32::System::Threading::AttachThreadInput(me, fg_thread, false);
        }
    }
}
