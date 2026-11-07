//! Global hotkeys: parse "Ctrl+Shift+PrintScreen" and register it with `RegisterHotKey`.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub mods: HOT_KEY_MODIFIERS,
    pub vk: u32,
}

fn key_code(name: &str) -> Option<u32> {
    let n = name.trim();
    let upper = n.to_ascii_uppercase();
    let named = match upper.as_str() {
        "PRINTSCREEN" | "PRTSC" | "PRTSCN" | "PRINT" | "SNAPSHOT" => 0x2C,
        "SPACE" => 0x20,
        "INSERT" | "INS" => 0x2D,
        "DELETE" | "DEL" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" | "PGUP" => 0x21,
        "PAGEDOWN" | "PGDN" => 0x22,
        "PAUSE" => 0x13,
        "SCROLLLOCK" => 0x91,
        "TAB" => 0x09,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    if let Some(num) = upper.strip_prefix('F').and_then(|d| d.parse::<u32>().ok()) {
        if (1..=24).contains(&num) {
            return Some(0x70 + num - 1);
        }
    }
    let mut chars = upper.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => Some(c as u32),
        _ => None,
    }
}

/// Parse "Ctrl+Shift+S". Empty text means "no hotkey" and returns `Ok(None)`.
pub fn parse(text: &str) -> Result<Option<Chord>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let mut mods = MOD_NOREPEAT;
    let parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let (key, modifiers) = parts.split_last().ok_or("empty shortcut")?;
    for m in modifiers {
        mods |= match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => MOD_CONTROL,
            "shift" => MOD_SHIFT,
            "alt" => MOD_ALT,
            "win" | "super" | "meta" => MOD_WIN,
            other => return Err(format!("unknown modifier \"{other}\" in \"{text}\"")),
        };
    }
    let vk = key_code(key).ok_or_else(|| format!("unknown key \"{key}\" in \"{text}\""))?;
    Ok(Some(Chord { mods, vk }))
}

/// The name the settings file uses for a virtual key, for the keys `parse` understands.
pub fn name_from_vk(vk: u32) -> Option<String> {
    let name = match vk {
        0x2C => "PrintScreen",
        0x20 => "Space",
        0x2D => "Insert",
        0x2E => "Delete",
        0x24 => "Home",
        0x23 => "End",
        0x21 => "PageUp",
        0x22 => "PageDown",
        0x13 => "Pause",
        0x91 => "ScrollLock",
        0x09 => "Tab",
        0x70..=0x87 => return Some(format!("F{}", vk - 0x70 + 1)),
        0x30..=0x39 | 0x41..=0x5A => return char::from_u32(vk).map(String::from),
        _ => return None,
    };
    Some(name.to_string())
}

/// Modifier keys held with a recorded key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

/// Keys that can be a shortcut on their own, or with only Shift: none of them types anything.
fn stands_alone(key: &str) -> bool {
    matches!(key, "PrintScreen" | "Pause" | "ScrollLock")
        || key.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| (13..=24).contains(&n))
}

/// The settings text for keys pressed in the shortcut recorder, as "Ctrl+Alt+Shift+Win+Key", or
/// why they can't be a shortcut.
pub fn recorded(held: Held, vk: u32) -> Result<String, String> {
    let key = name_from_vk(vk).ok_or("That key can't be a shortcut. Try a letter, a number or Print Screen.")?;
    if !(held.ctrl || held.alt || held.win) && !stands_alone(&key) {
        return Err("Add Ctrl, Alt or Win, so that typing still works.".to_string());
    }
    let mut parts = Vec::new();
    for (on, name) in [(held.ctrl, "Ctrl"), (held.alt, "Alt"), (held.shift, "Shift"), (held.win, "Win")] {
        if on {
            parts.push(name);
        }
    }
    parts.push(&key);
    Ok(parts.join("+"))
}

/// Whether two shortcut texts mean the same keys, however they're written.
pub fn same(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Ok(Some(x)), Ok(Some(y))) => x == y,
        (Ok(None), Ok(None)) => true,
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    }
}

/// A key's name as its key cap shows it.
pub fn cap_label(part: &str) -> &str {
    match part {
        "PrintScreen" => "PrtSc",
        "ScrollLock" => "ScrLk",
        "PageUp" => "PgUp",
        "PageDown" => "PgDn",
        other => other,
    }
}

/// Register `chord` under `id`. Returns a message for the user on failure.
pub fn register(hwnd: HWND, id: i32, text: &str) -> Result<(), String> {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), id);
    }
    match parse(text)? {
        None => Ok(()),
        Some(c) => unsafe { RegisterHotKey(Some(hwnd), id, c.mods, c.vk) }.map_err(|_| format!("{text} is already used by another app")),
    }
}

pub fn unregister(hwnd: HWND, id: i32) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_shortcuts() {
        assert_eq!(parse("PrintScreen").unwrap(), Some(Chord { mods: MOD_NOREPEAT, vk: 0x2C }));
        assert_eq!(parse("ctrl + shift + s").unwrap(), Some(Chord { mods: MOD_NOREPEAT | MOD_CONTROL | MOD_SHIFT, vk: 'S' as u32 }));
        assert_eq!(parse("Win+F12").unwrap(), Some(Chord { mods: MOD_NOREPEAT | MOD_WIN, vk: 0x7B }));
        assert_eq!(parse("").unwrap(), None);
        assert!(parse("Hyper+S").is_err());
        assert!(parse("Ctrl+Foo").is_err());
    }

    #[test]
    fn recorded_keys_read_back_as_the_same_shortcut() {
        let ctrl_alt = Held { ctrl: true, alt: true, ..Held::default() };
        assert_eq!(recorded(ctrl_alt, 'S' as u32).unwrap(), "Ctrl+Alt+S");
        let all = Held { ctrl: true, alt: true, shift: true, win: true };
        assert_eq!(recorded(all, 0x2C).unwrap(), "Ctrl+Alt+Shift+Win+PrintScreen");
        for vk in (0x30..=0x39).chain(0x41..=0x5A).chain(0x70..=0x87).chain([0x2C, 0x20, 0x2D, 0x2E, 0x24, 0x23, 0x21, 0x22, 0x13, 0x91, 0x09]) {
            let text = recorded(ctrl_alt, vk).unwrap();
            assert_eq!(parse(&text).unwrap().map(|c| c.vk), Some(vk), "{text}");
        }
    }

    #[test]
    fn a_recorded_shortcut_must_leave_typing_alone() {
        let none = Held::default();
        let shift = Held { shift: true, ..Held::default() };
        assert_eq!(recorded(none, 0x2C).unwrap(), "PrintScreen");
        assert_eq!(recorded(shift, 0x2C).unwrap(), "Shift+PrintScreen");
        assert_eq!(recorded(none, 0x7C).unwrap(), "F13");
        assert!(recorded(none, 'S' as u32).is_err());
        assert!(recorded(shift, 'S' as u32).is_err(), "Shift+S types a capital S");
        assert!(recorded(none, 0x70).is_err(), "F1 is help in most apps");
        assert!(recorded(Held { ctrl: true, ..Held::default() }, 0xBA).is_err(), "keys parse can't read back");
    }

    #[test]
    fn shortcuts_compare_by_their_keys() {
        assert!(same("Ctrl+Alt+S", "alt + ctrl + s"));
        assert!(same("", " "));
        assert!(!same("Ctrl+S", "Ctrl+Shift+S"));
    }
}
