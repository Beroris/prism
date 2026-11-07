//! Takes Win+Shift+S away from Snipping Tool.
//!
//! Windows handles Win+Shift+S inside the shell, so `RegisterHotKey` can't claim it and there is
//! no setting to turn it off. A low-level keyboard hook sees the key before the shell does: it
//! swallows the S, sends a harmless "mask" key so that releasing Win doesn't open the Start
//! menu, and posts a capture request to the main window.
//!
//! The hook is only installed while the option is on. Per keystroke it does a few comparisons
//! and keeps no record of what is typed. Windows doesn't send keys from elevated (admin) windows
//! to it, so there Snipping Tool still gets the shortcut.

use crate::util::SendHwnd;
use std::cell::Cell;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
    VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// Marks keys we inject ourselves, so the hook lets them through.
const INJECTED_TAG: usize = 0x5348_4F54;
/// An unassigned virtual key: pressing it between Win down and Win up stops the Start menu.
const MASK_VK: u16 = 0xE8;
const VK_S: u32 = 0x53;

thread_local! {
    static HOOK: Cell<Option<HHOOK>> = const { Cell::new(None) };
    static TARGET: Cell<Option<(SendHwnd, u32)>> = const { Cell::new(None) };
    /// True from swallowing an S press until its release, so repeats and the key-up are eaten too.
    static SWALLOWING: Cell<bool> = const { Cell::new(false) };
}

fn down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

/// Whether Win+Shift+S (and only those modifiers) is held for this S press.
pub fn is_snip_chord(vk: u32, win: bool, shift: bool, ctrl: bool, alt: bool) -> bool {
    vk == VK_S && win && shift && !ctrl && !alt
}

fn mask_start_menu() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(MASK_VK), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: INJECTED_TAG } },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

unsafe extern "system" fn proc_(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code != HC_ACTION as i32 {
            return CallNextHookEx(None, code, wp, lp);
        }
        let ev = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        if ev.dwExtraInfo == INJECTED_TAG || ev.vkCode != VK_S {
            return CallNextHookEx(None, code, wp, lp);
        }
        let msg = wp.0 as u32;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        if SWALLOWING.get() {
            if is_up {
                SWALLOWING.set(false);
            }
            return LRESULT(1);
        }
        let win = down(VK_LWIN.0) || down(VK_RWIN.0);
        if is_down && is_snip_chord(ev.vkCode, win, down(VK_SHIFT.0), down(VK_CONTROL.0), down(VK_MENU.0)) {
            SWALLOWING.set(true);
            mask_start_menu();
            if let Some((hwnd, msg)) = TARGET.get() {
                hwnd.post_boxed(msg, 0, ());
            }
            return LRESULT(1);
        }
        CallNextHookEx(None, code, wp, lp)
    }
}

/// Install or remove the hook. `notify` receives `msg` (with a boxed `()` in lParam) each time
/// Win+Shift+S is pressed. Must run on the thread with the message loop.
pub fn set_enabled(on: bool, notify: SendHwnd, msg: u32) -> bool {
    let current = HOOK.get();
    match (on, current) {
        (true, Some(_)) => true,
        (true, None) => {
            TARGET.set(Some((notify, msg)));
            match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc_), None, 0) } {
                Ok(h) => {
                    HOOK.set(Some(h));
                    true
                }
                Err(_) => false,
            }
        }
        (false, Some(h)) => {
            unsafe {
                let _ = UnhookWindowsHookEx(h);
            }
            HOOK.set(None);
            SWALLOWING.set(false);
            true
        }
        (false, None) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_chord_matches() {
        assert!(is_snip_chord(VK_S, true, true, false, false));
        assert!(!is_snip_chord(VK_S, true, false, false, false)); // Win+S is search
        assert!(!is_snip_chord(VK_S, false, true, false, false));
        assert!(!is_snip_chord(VK_S, true, true, true, false));
        assert!(!is_snip_chord(VK_S, true, true, false, true));
        assert!(!is_snip_chord(0x41, true, true, false, false));
    }
}
