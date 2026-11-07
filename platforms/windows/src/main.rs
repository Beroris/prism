//! Prism for Windows.
//!
//! `prism.exe [--region | --screen | --text | --settings | --quit] [--background]`
//!
//! One resident tray process. Starting the exe again forwards the command to it (no argument
//! means a region capture), so other tools — Orbit, a Stream Deck, a shortcut — can trigger
//! captures without a second process staying around.

#![windows_subsystem = "windows"]

mod app;
mod capture;
mod clipboard;
mod hotkey;
mod keyhook;
mod ocr;
mod overlay;
mod settings_win;
mod system;
mod text;
mod toast;
mod tray;
mod ui;
mod util;

use app::{COPYDATA_TAG, Command, MAIN_CLASS};
use util::wide;
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, DispatchMessageW, FindWindowW, GetMessageW, GetWindowThreadProcessId, MSG, SendMessageTimeoutW,
    SMTO_ABORTIFHUNG, TranslateMessage, WM_COPYDATA,
};
use windows::core::PCWSTR;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Some(i) = args.iter().position(|a| a == "--selftest-render") {
        let script = args.get(i + 1).cloned().unwrap_or_default();
        let out = args.get(i + 2).cloned().unwrap_or_else(|| "overlay.png".into());
        std::process::exit(selftest_render(&script, &out));
    }

    if let Some(i) = args.iter().position(|a| a == "--preview-settings") {
        // --preview-settings <page>[-recording|-problem|-saved] <out.png> : render Settings off-screen.
        let what = args.get(i + 1).cloned().unwrap_or_default();
        let out = args.get(i + 2).cloned().unwrap_or_else(|| "settings.png".into());
        std::process::exit(settings_win::preview(&what, std::path::Path::new(&out)));
    }

    if let Some(i) = args.iter().position(|a| a == "--selftest-ocr") {
        // --selftest-ocr x y w h out.txt : read text from a screen region into a file.
        let n: Vec<i32> = args[i + 1..].iter().take(4).filter_map(|s| s.parse().ok()).collect();
        let out = args.get(i + 5).cloned().unwrap_or_else(|| "ocr.txt".into());
        let Some(img) = (n.len() == 4).then(|| capture::grab(prism_core::IRect::new(n[0], n[1], n[2], n[3]), false)).flatten() else {
            std::process::exit(2)
        };
        let t = std::time::Instant::now();
        let r = ocr::recognize(&img);
        let _ = std::fs::write(&out, format!("{:?}\n{}", t.elapsed(), r.unwrap_or_else(|e| format!("ERROR {e}"))));
        std::process::exit(0);
    }

    let command = args.iter().find_map(|a| Command::parse(a));
    let background = args.iter().any(|a| a == "--background");

    // One instance. A second start forwards its command and exits.
    let mutex_name = wide("Local\\Prism.Instance");
    let _mutex = unsafe { CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr())) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if !background {
            forward(command.unwrap_or(Command::Region));
        }
        return;
    }
    if command == Some(Command::Quit) {
        return;
    }

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }
    let Some(hwnd) = app::create_window() else { return };
    app::start(hwnd, command, background);

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if settings_win::eats_key(&msg) {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Hand a command to the running instance.
fn forward(cmd: Command) {
    let class = wide(MAIN_CLASS);
    unsafe {
        let Ok(target) = FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) else { return };
        // We were just started by the user, so we may give focus away: let the running instance
        // bring its overlay to the front.
        let mut pid = 0u32;
        GetWindowThreadProcessId(target, Some(&mut pid));
        let _ = AllowSetForegroundWindow(pid);
        let payload = cmd.as_str().as_bytes();
        let cds = COPYDATASTRUCT { dwData: COPYDATA_TAG, cbData: payload.len() as u32, lpData: payload.as_ptr() as *mut _ };
        let _ = SendMessageTimeoutW(target, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize), SMTO_ABORTIFHUNG, 2000, None);
    }
}

/// Render the overlay offscreen to a PNG, for checking the UI without taking over the screen.
fn selftest_render(script: &str, out: &str) -> i32 {
    let t = std::time::Instant::now();
    let Some(img) = overlay::render_offscreen(script) else { return 2 };
    let elapsed = t.elapsed();
    let Ok(png) = prism_core::image::encode_png(&img) else { return 3 };
    if std::fs::write(out, png).is_err() {
        return 4;
    }
    let _ = std::fs::write(format!("{out}.txt"), format!("render {:?}\n", elapsed));
    0
}
