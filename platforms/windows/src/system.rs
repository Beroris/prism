//! Registry-backed bits of system integration: start with Windows, the Windows 11 setting
//! that gives the Print Screen key to Snipping Tool, and Explorer's Win+S shortcuts. Plus the
//! standard dialogs.

use crate::util::{APP_NAME, wide, wide_path};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileOpenDialog, IFileSaveDialog,
    IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONQUESTION, MB_YESNO, IDYES, MessageBoxW};
use windows::core::PCWSTR;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const KEYBOARD_KEY: &str = "Control Panel\\Keyboard";
const SNIP_VALUE: &str = "PrintScreenKeyForSnippingEnabled";

fn open_key(path: &str, access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Option<HKEY> {
    let p = wide(path);
    let mut key = HKEY::default();
    unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr()), Some(0), access, &mut key).ok().ok()? };
    Some(key)
}

fn read_value(path: &str, name: &str) -> Option<(REG_VALUE_TYPE, Vec<u8>)> {
    let key = open_key(path, KEY_READ)?;
    let n = wide(name);
    let mut ty = REG_VALUE_TYPE::default();
    let mut buf = vec![0u8; 2048];
    let mut len = buf.len() as u32;
    let r = unsafe { RegQueryValueExW(key, PCWSTR(n.as_ptr()), None, Some(&mut ty), Some(buf.as_mut_ptr()), Some(&mut len)) };
    unsafe {
        let _ = RegCloseKey(key);
    }
    r.ok().ok()?;
    buf.truncate(len as usize);
    Some((ty, buf))
}

fn exe_command() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    format!("\"{}\" --background", exe.display())
}

pub fn autostart_enabled() -> bool {
    match read_value(RUN_KEY, APP_NAME) {
        Some((_, bytes)) => {
            let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
            // Only "on" if it points at this exe; a stale entry from a moved exe counts as off.
            String::from_utf16_lossy(&units).eq_ignore_ascii_case(&exe_command())
        }
        None => false,
    }
}

pub fn set_autostart(on: bool) {
    let Some(key) = open_key(RUN_KEY, KEY_SET_VALUE) else { return };
    let name = wide(APP_NAME);
    unsafe {
        if on {
            let cmd = wide(&exe_command());
            let bytes = std::slice::from_raw_parts(cmd.as_ptr() as *const u8, cmd.len() * 2);
            let _ = RegSetValueExW(key, PCWSTR(name.as_ptr()), Some(0), REG_SZ, Some(bytes));
        } else {
            let _ = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
        }
        let _ = RegCloseKey(key);
    }
}

/// Whether Windows sends Print Screen to Snipping Tool. Missing means on: that has been the
/// Windows 11 default since 2023.
pub fn snipping_owns_printscreen() -> bool {
    match read_value(KEYBOARD_KEY, SNIP_VALUE) {
        Some((ty, b)) if ty == REG_DWORD && b.len() >= 4 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) != 0,
        Some(_) => true,
        None => windows_build() >= 22000,
    }
}

/// Give Print Screen to Snipping Tool (`true`) or take it back (`false`).
pub fn set_snipping_printscreen(snipping: bool) -> bool {
    let Some(key) = open_key(KEYBOARD_KEY, KEY_SET_VALUE) else { return false };
    let name = wide(SNIP_VALUE);
    let value = (snipping as u32).to_le_bytes();
    let ok = unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), Some(0), REG_DWORD, Some(&value)).is_ok() };
    unsafe {
        let _ = RegCloseKey(key);
    }
    ok
}

/// The letters whose Win shortcuts Explorer leaves alone, as a string such as "SV".
const ADVANCED_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced";
const DISABLED_HOTKEYS: &str = "DisabledHotkeys";

/// `letters` with `letter` added (`on`) or taken out, leaving the others as they were.
fn with_letter(letters: &str, letter: char, on: bool) -> String {
    let mut kept: String = letters.chars().filter(|c| !c.eq_ignore_ascii_case(&letter)).collect();
    if on {
        kept.push(letter);
    }
    kept
}

/// Make Explorer leave the Win+S shortcuts alone (`true`), Win+Shift+S included, so nothing
/// opens Snipping Tool and Prism can register the key, even over admin windows. Or let Explorer
/// have them again (`false`). Explorer reads this when it starts, so it takes effect at the next
/// sign-in or Explorer restart.
pub fn set_explorer_s_keys_off(off: bool) -> bool {
    let now = read_value(ADVANCED_KEY, DISABLED_HOTKEYS)
        .filter(|(ty, _)| *ty == REG_SZ)
        .map(|(_, b)| {
            let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
            String::from_utf16_lossy(&units)
        })
        .unwrap_or_default();
    let next = with_letter(&now, 'S', off);
    if next == now {
        return true;
    }
    let Some(key) = open_key(ADVANCED_KEY, KEY_SET_VALUE) else { return false };
    let name = wide(DISABLED_HOTKEYS);
    let ok = unsafe {
        if next.is_empty() {
            RegDeleteValueW(key, PCWSTR(name.as_ptr())).is_ok()
        } else {
            let text = wide(&next);
            let bytes = std::slice::from_raw_parts(text.as_ptr() as *const u8, text.len() * 2);
            RegSetValueExW(key, PCWSTR(name.as_ptr()), Some(0), REG_SZ, Some(bytes)).is_ok()
        }
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    ok
}

fn windows_build() -> u32 {
    read_value("Software\\Microsoft\\Windows NT\\CurrentVersion", "CurrentBuildNumber")
        .and_then(|(_, b)| {
            let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
            String::from_utf16_lossy(&units).parse().ok()
        })
        .unwrap_or(0)
}

pub fn ask(hwnd: HWND, text: &str) -> bool {
    let t = wide(text);
    let c = wide(APP_NAME);
    unsafe { MessageBoxW(Some(hwnd), PCWSTR(t.as_ptr()), PCWSTR(c.as_ptr()), MB_YESNO | MB_ICONQUESTION) == IDYES }
}

fn item_path(item: &IShellItem) -> Option<PathBuf> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

fn folder_item(dir: &Path) -> Option<IShellItem> {
    let w = wide_path(dir);
    unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok() }
}

/// "Save as" for a PNG. Returns the chosen path.
pub fn save_dialog(owner: HWND, dir: &Path, name: &str) -> Option<PathBuf> {
    unsafe {
        let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filter_name = wide("PNG image");
        let filter_spec = wide("*.png");
        let filters = [COMDLG_FILTERSPEC { pszName: PCWSTR(filter_name.as_ptr()), pszSpec: PCWSTR(filter_spec.as_ptr()) }];
        let _ = d.SetFileTypes(&filters);
        let ext = wide("png");
        let _ = d.SetDefaultExtension(PCWSTR(ext.as_ptr()));
        let n = wide(name);
        let _ = d.SetFileName(PCWSTR(n.as_ptr()));
        if let Some(folder) = folder_item(dir) {
            let _ = d.SetFolder(&folder);
        }
        let opts = d.GetOptions().unwrap_or_default();
        let _ = d.SetOptions(opts | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM);
        d.Show(Some(owner)).ok()?;
        item_path(&d.GetResult().ok()?)
    }
}

pub fn pick_folder(owner: HWND, start: &Path) -> Option<PathBuf> {
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = d.GetOptions().unwrap_or_default();
        let _ = d.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
        if let Some(folder) = folder_item(start) {
            let _ = d.SetFolder(&folder);
        }
        let title = wide("Choose where screenshots are saved");
        let _ = d.SetTitle(PCWSTR(title.as_ptr()));
        d.Show(Some(owner)).ok()?;
        item_path(&d.GetResult().ok()?)
    }
}

/// A text value under `HKEY_CURRENT_USER\path`.
pub fn read_string(path: &str, name: &str) -> Option<String> {
    let (ty, bytes) = read_value(path, name)?;
    if ty != REG_SZ && ty != windows::Win32::System::Registry::REG_EXPAND_SZ {
        return None;
    }
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    Some(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::with_letter;

    #[test]
    fn letters_keep_the_others() {
        assert_eq!(with_letter("", 'S', true), "S");
        assert_eq!(with_letter("V", 'S', true), "VS");
        assert_eq!(with_letter("SV", 'S', true), "VS");
        assert_eq!(with_letter("sV", 'S', false), "V");
        assert_eq!(with_letter("S", 'S', false), "");
        assert_eq!(with_letter("V", 'S', false), "V");
    }
}
