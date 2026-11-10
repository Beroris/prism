//! Bringing settings over from other screenshot apps: ShareX, Greenshot, Flameshot and Lightshot.
//!
//! The host finds each app's settings files and reads them. This turns their text into the
//! Prism settings they mean (`Found`), and puts those into Prism's `Settings` (`Found::apply`),
//! saying in words what changed. Nothing here touches the disk: folder variables in paths, such
//! as ShareX's `%MyDocuments%` or Greenshot's `${MyPictures}`, are looked up through a function
//! the host passes in.
//!
//! Only what the other app was set to do comes over, and only where Prism can do the same.
//! Anything it can't (uploads, window titles in file names, a shortcut Prism can't register) is
//! left as Prism has it.

use crate::color::Rgba;
use crate::doc::WIDTHS;
use crate::naming::{Stamp, format_name};
use crate::settings::{EnterAction, Hotkeys, Settings, is_print_screen};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum App {
    ShareX,
    Greenshot,
    Flameshot,
    Lightshot,
}

impl App {
    pub const ALL: [App; 4] = [App::ShareX, App::Greenshot, App::Flameshot, App::Lightshot];

    pub fn name(self) -> &'static str {
        match self {
            App::ShareX => "ShareX",
            App::Greenshot => "Greenshot",
            App::Flameshot => "Flameshot",
            App::Lightshot => "Lightshot",
        }
    }
}

/// Looks up a folder or environment variable by name, such as `MyPictures` or `USERPROFILE`.
pub type Vars<'a> = &'a dyn Fn(&str) -> Option<String>;

/// What another app is set to, in Prism's terms. `None` means it isn't set there, or Prism can't
/// do the same.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    /// Print Screen alone captures a region there, which Prism does with Replace Snipping Tool.
    pub print_screen: bool,
    /// Shortcuts, as Prism writes them, in the settings window's order: capture a region,
    /// capture the screen, copy text, capture the last region again.
    pub shortcuts: [Option<String>; 4],
    pub save_dir: Option<String>,
    pub file_name: Option<String>,
    pub enter_action: Option<EnterAction>,
    pub save_copies: Option<bool>,
    pub capture_cursor: Option<bool>,
    pub magnifier: Option<bool>,
    pub notify: Option<bool>,
    pub color: Option<Rgba>,
    pub width: Option<f32>,
    pub palette: Option<Vec<Rgba>>,
}

/// What each of Prism's shortcuts does, as the end of a sentence that starts with its keys.
const DOES: [&str; 5] = [
    "captures a region",
    "captures the screen",
    "copies text from the screen",
    "captures the last region again",
    "marks up the last region again",
];
/// Each of Prism's shortcuts, as the start of a clause.
const NAMES: [&str; 5] = ["capturing a region", "capturing the screen", "copying text", "capturing the last region again", "marking up the last region again"];

fn shortcut_mut(keys: &mut Hotkeys, slot: usize) -> &mut String {
    match slot {
        0 => &mut keys.region,
        1 => &mut keys.screen,
        2 => &mut keys.text,
        3 => &mut keys.last,
        _ => &mut keys.last_markup,
    }
}

/// A shortcut as it reads in a sentence.
fn spoken(keys: &str) -> String {
    keys.replace("PrintScreen", "Print Screen")
}

impl Found {
    /// Puts a shortcut in its slot, unless one is there already. Print Screen alone for a region
    /// becomes `print_screen`.
    fn put(&mut self, slot: usize, keys: String) {
        if slot == 0 && is_print_screen(&keys) {
            self.print_screen = true;
        } else if self.shortcuts[slot].is_none() {
            self.shortcuts[slot] = Some(keys);
        }
    }

    /// Whether there's anything at all to bring over.
    pub fn is_empty(&self) -> bool {
        *self == Found::default()
    }

    /// Brings everything over into `s`, and says what changed, one short clause each, in the
    /// order a person would check them. `example` dates the example file name.
    ///
    /// The other app's shortcuts win: one of Prism's that has the same keys is turned off, and
    /// that's said too. Print Screen alone is only ever for capturing a region (with Replace
    /// Snipping Tool), as it is in Prism.
    pub fn apply(&self, s: &mut Settings, example: Stamp) -> Vec<String> {
        let mut said = Vec::new();
        if self.print_screen && !s.replace_snipping {
            s.replace_snipping = true;
            said.push("Print Screen captures a region".to_string());
        }
        let mut claimed = [false; 5];
        let mut claimed_keys: Vec<&str> = Vec::new();
        for (slot, keys) in self.shortcuts.iter().enumerate() {
            let Some(keys) = keys else { continue };
            let reserved = is_print_screen(keys) || (s.replace_snipping && same_keys(keys, "Win+Shift+S"));
            if reserved || claimed_keys.iter().any(|k| same_keys(k, keys)) {
                continue;
            }
            claimed[slot] = true;
            claimed_keys.push(keys);
            let current = shortcut_mut(&mut s.hotkeys, slot);
            if !same_keys(current, keys) {
                *current = keys.clone();
                said.push(format!("{} {}", spoken(keys), DOES[slot]));
            }
        }
        for slot in 0..DOES.len() {
            let current = shortcut_mut(&mut s.hotkeys, slot);
            if !claimed[slot] && !current.trim().is_empty() && claimed_keys.iter().any(|k| same_keys(k, current)) {
                current.clear();
                said.push(format!("{} loses its shortcut", NAMES[slot]));
            }
        }
        if let Some(dir) = &self.save_dir {
            let tidy = |d: &str| d.trim().trim_end_matches(['\\', '/']).to_ascii_lowercase();
            if tidy(dir) != tidy(&s.save_dir) {
                s.save_dir = dir.clone();
                said.push(format!("captures are saved in {dir}"));
            }
        }
        if let Some(name) = &self.file_name
            && *name != s.file_name
        {
            s.file_name = name.clone();
            said.push(format!("files are named like \u{201C}{}\u{201D}", format_name(name, example, 1920, 1080)));
        }
        if let Some(action) = self.enter_action
            && action != s.enter_action
        {
            s.enter_action = action;
            said.push(
                match action {
                    EnterAction::Copy => "finishing copies",
                    EnterAction::Save => "finishing saves",
                    EnterAction::CopyAndSave => "finishing copies and saves",
                }
                .to_string(),
            );
        }
        let mut switch = |value: Option<bool>, setting: &mut bool, on: &str, off: &str| {
            if let Some(v) = value
                && v != *setting
            {
                *setting = v;
                said.push(if v { on } else { off }.to_string());
            }
        };
        switch(self.save_copies, &mut s.save_copies, "copied captures are saved too", "copied captures aren't saved");
        switch(self.capture_cursor, &mut s.capture_cursor, "the mouse pointer is included", "the mouse pointer is left out");
        switch(self.magnifier, &mut s.magnifier, "the magnifier shows", "the magnifier is hidden");
        switch(self.notify, &mut s.notify, "a popup shows after copying or saving", "no popup shows after copying or saving");
        if let Some(color) = self.color {
            let now = s.start_color.unwrap_or(s.style.color);
            if color != now {
                s.style.color = color;
                if s.start_color.is_some() {
                    s.start_color = Some(color);
                }
                said.push("you draw in its pen colour".to_string());
            }
        }
        if let Some(width) = self.width.map(nearest_width) {
            let now = s.start_width.unwrap_or(s.style.width);
            if (width - now).abs() > 0.01 {
                s.style.width = width;
                if s.start_width.is_some() {
                    s.start_width = Some(width);
                }
                said.push(format!("the pen size is {}", width as i32));
            }
        }
        if let Some(palette) = &self.palette
            && !palette.is_empty()
            && *palette != s.palette
        {
            s.palette = palette.iter().take(12).copied().collect();
            said.push("its colours fill the palette".to_string());
        }
        said
    }

    /// What `apply` would change, without changing anything.
    pub fn changes(&self, s: &Settings, example: Stamp) -> Vec<String> {
        self.apply(&mut s.clone(), example)
    }
}

/// Puts back everything `Found::apply` can change, as it was in `before`, and leaves the rest
/// (such as the last region, or the tool last used) as it is now.
pub fn undo(before: &Settings, s: &mut Settings) {
    s.hotkeys = before.hotkeys.clone();
    s.replace_snipping = before.replace_snipping;
    s.save_dir = before.save_dir.clone();
    s.file_name = before.file_name.clone();
    s.enter_action = before.enter_action;
    s.save_copies = before.save_copies;
    s.capture_cursor = before.capture_cursor;
    s.magnifier = before.magnifier;
    s.notify = before.notify;
    s.style.color = before.style.color;
    s.style.width = before.style.width;
    s.start_color = before.start_color;
    s.start_width = before.start_width;
    s.palette = before.palette.clone();
}

/// Clauses joined into one sentence: "A, b and c."
pub fn sentence(parts: &[String]) -> String {
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push_str(if i + 1 == parts.len() { " and " } else { ", " });
        }
        out.push_str(part);
    }
    let mut chars = out.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn nearest_width(w: f32) -> f32 {
    WIDTHS.iter().copied().min_by(|a, b| (a - w).abs().total_cmp(&(b - w).abs())).unwrap_or(w)
}

// --- Shortcuts --------------------------------------------------------------------------------

/// A key's name as Prism's settings write it, from the names other apps use.
fn key_name(key: &str) -> Option<String> {
    let lower = key.trim().to_ascii_lowercase();
    let named = match lower.as_str() {
        "printscreen" | "snapshot" | "print" | "prtsc" | "prtscn" => "PrintScreen",
        "space" => "Space",
        "insert" | "ins" => "Insert",
        "delete" | "del" => "Delete",
        "home" => "Home",
        "end" => "End",
        "pageup" | "prior" | "pgup" => "PageUp",
        "pagedown" | "next" | "pgdown" | "pgdn" => "PageDown",
        "pause" => "Pause",
        "scroll" | "scrolllock" => "ScrollLock",
        "tab" => "Tab",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_string());
    }
    let mut chars = lower.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(c), None, None) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase().to_string()),
        // .NET writes the number keys as D0 to D9.
        (Some('d'), Some(n), None) if n.is_ascii_digit() => Some(n.to_string()),
        _ => {
            let n = lower.strip_prefix('f')?.parse::<u32>().ok()?;
            (1..=24).contains(&n).then(|| format!("F{n}"))
        }
    }
}

/// A shortcut as Prism writes it, "Ctrl+Alt+Shift+Win+Key", or `None` if Prism can't use it:
/// an unknown key, or one that would get in the way of typing, by the same rule as recording a
/// shortcut in Settings.
fn chord(ctrl: bool, alt: bool, shift: bool, win: bool, key: &str) -> Option<String> {
    let key = key_name(key)?;
    let alone = matches!(key.as_str(), "PrintScreen" | "Pause" | "ScrollLock")
        || key.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| (13..=24).contains(&n));
    if !(ctrl || alt || win || alone) {
        return None;
    }
    let mut parts: Vec<&str> = [(ctrl, "Ctrl"), (alt, "Alt"), (shift, "Shift"), (win, "Win")].iter().filter(|(on, _)| *on).map(|(_, name)| *name).collect();
    parts.push(&key);
    Some(parts.join("+"))
}

/// Shortcuts written with `+`: Prism's and Flameshot's ("Ctrl+Shift+x", where Meta is Win).
fn plus_shortcut(text: &str) -> Option<String> {
    let parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let (key, mods) = parts.split_last()?;
    let (mut ctrl, mut alt, mut shift, mut win) = (false, false, false, false);
    for m in mods {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" => alt = true,
            "shift" => shift = true,
            "win" | "windows" | "meta" | "super" => win = true,
            _ => return None,
        }
    }
    chord(ctrl, alt, shift, win, key)
}

/// Whether two shortcuts are the same keys, however they're written.
fn same_keys(a: &str, b: &str) -> bool {
    match (plus_shortcut(a), plus_shortcut(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    }
}

/// ShareX: the .NET key with its modifiers as flags, "PrintScreen, Control", and Win apart.
fn sharex_shortcut(hotkey: &str, win: bool) -> Option<String> {
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    let mut key = None;
    for part in hotkey.split(',').map(str::trim) {
        match part {
            "Control" => ctrl = true,
            "Alt" => alt = true,
            "Shift" => shift = true,
            "" | "None" => {}
            other if key.is_none() => key = Some(other),
            _ => return None,
        }
    }
    chord(ctrl, alt, shift, win, key?)
}

/// Greenshot: "Alt + Ctrl + PrintScreen". Like Greenshot, the modifiers are found by name
/// anywhere before the key, and the key is what follows the last `+`.
fn greenshot_shortcut(text: &str) -> Option<String> {
    let (mods, key) = text.rsplit_once('+').unwrap_or(("", text));
    let mods = mods.to_ascii_lowercase();
    chord(mods.contains("ctrl"), mods.contains("alt"), mods.contains("shift"), mods.contains("win"), key)
}

// --- File names -------------------------------------------------------------------------------

/// A file name pattern, read into parts.
enum Piece {
    Text(String),
    /// One of Prism's placeholders, without its braces.
    Token(&'static str),
    /// Something Prism can't fill in, such as the window title or a random number.
    Gone,
}

const SEPARATORS: &[char] = &[' ', '-', '_', '.', ','];

/// The parts as a Prism pattern. What Prism can't fill in goes, with the separator before it (or
/// after it, at the start). Without a date, a time or the size left, nothing comes over, as every
/// file would be named the same.
fn assemble(pieces: Vec<Piece>) -> Option<String> {
    let mut merged: Vec<Piece> = Vec::new();
    for piece in pieces {
        match (merged.last_mut(), piece) {
            (Some(Piece::Text(a)), Piece::Text(b)) => a.push_str(&b),
            (_, piece) => merged.push(piece),
        }
    }
    for i in 0..merged.len() {
        if !matches!(merged[i], Piece::Gone) {
            continue;
        }
        let before = i > 0
            && match &mut merged[i - 1] {
                Piece::Text(t) => {
                    let kept = t.trim_end_matches(SEPARATORS).len();
                    t.truncate(kept);
                    true
                }
                _ => false,
            };
        if !before
            && let Some(Piece::Text(t)) = merged.get_mut(i + 1)
        {
            *t = t.trim_start_matches(SEPARATORS).to_string();
        }
    }
    if !merged.iter().any(|p| matches!(p, Piece::Token(_))) {
        return None;
    }
    let mut out = String::new();
    for piece in &merged {
        match piece {
            Piece::Text(t) => out.push_str(t),
            Piece::Token(t) => {
                out.push('{');
                out.push_str(t);
                out.push('}');
            }
            Piece::Gone => {}
        }
    }
    let out = out.trim_matches(SEPARATORS).replace("{yyyy}-{MM}-{dd}", "{date}");
    (!out.is_empty()).then_some(out)
}

/// ShareX's name pattern (`%y-%mo-%d_%h-%mi-%s`).
fn sharex_name(pattern: &str) -> Option<String> {
    // Longest first, so "%mon" isn't read as "%mo" and "%height" as "%h".
    const TOKENS: [(&str, Option<&str>); 43] = [
        ("%radjective", None),
        ("%ranimal", None),
        ("%remoji", None),
        ("%height", Some("h")),
        ("%width", Some("w")),
        ("%unix", None),
        ("%guid", None),
        ("%GUID", None),
        ("%mon2", None),
        ("%iAa", None),
        ("%iaA", None),
        ("%mon", None),
        ("%uln", None),
        ("%rna", None),
        ("%rf", None),
        ("%rn", None),
        ("%ra", None),
        ("%rx", None),
        ("%rX", None),
        ("%ms", None),
        ("%mo", Some("MM")),
        ("%mi", Some("mm")),
        ("%yy", None),
        ("%wy", None),
        ("%w2", None),
        ("%pm", None),
        ("%pn", None),
        ("%un", None),
        ("%cn", None),
        ("%ia", None),
        ("%iA", None),
        ("%ib", None),
        ("%iB", None),
        ("%ix", None),
        ("%iX", None),
        ("%y", Some("yyyy")),
        ("%d", Some("dd")),
        ("%h", Some("HH")),
        ("%s", Some("ss")),
        ("%w", None),
        ("%t", None),
        ("%n", None),
        ("%i", None),
    ];
    let mut pieces = Vec::new();
    let mut rest = pattern;
    while !rest.is_empty() {
        let found = if rest.starts_with('%') { TOKENS.iter().find(|(t, _)| rest.starts_with(t)) } else { None };
        match found {
            Some((token, Some(prism))) => {
                pieces.push(Piece::Token(prism));
                rest = &rest[token.len()..];
            }
            Some((token, None)) => {
                pieces.push(Piece::Gone);
                rest = &rest[token.len()..];
                // Its argument, as in %ra{10}.
                if rest.starts_with('{') {
                    rest = rest.find('}').map_or("", |end| &rest[end + 1..]);
                }
            }
            None => {
                let c = rest.chars().next().unwrap_or_default();
                pieces.push(Piece::Text(c.to_string()));
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    assemble(pieces)
}

/// A .NET date format, such as `yyyy-MM-dd HH_mm_ss`, into parts.
fn dotnet_date(format: &str, out: &mut Vec<Piece>) {
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' || c == '"' {
            let end = chars[i + 1..].iter().position(|&q| q == c).map_or(chars.len(), |p| i + 1 + p);
            out.push(Piece::Text(chars[i + 1..end].iter().collect()));
            i = end + 1;
        } else if c == '\\' && i + 1 < chars.len() {
            out.push(Piece::Text(chars[i + 1].to_string()));
            i += 2;
        } else if c.is_ascii_alphabetic() {
            let run = chars[i..].iter().take_while(|&&x| x == c).count();
            out.push(match (c, run) {
                ('y', 3..) => Piece::Token("yyyy"),
                ('M', 1 | 2) => Piece::Token("MM"),
                ('d', 1 | 2) => Piece::Token("dd"),
                ('H' | 'h', 1 | 2) => Piece::Token("HH"),
                ('m', 1 | 2) => Piece::Token("mm"),
                ('s', 1 | 2) => Piece::Token("ss"),
                _ => Piece::Gone,
            });
            i += run;
        } else {
            if c != '%' {
                out.push(Piece::Text(c.to_string()));
            }
            i += 1;
        }
    }
}

/// Greenshot's name pattern (`${capturetime:d"yyyy-MM-dd HH_mm_ss"}-${title}`).
fn greenshot_name(pattern: &str) -> Option<String> {
    let mut pieces = Vec::new();
    let mut rest = pattern;
    while let Some(start) = rest.find("${") {
        pieces.push(Piece::Text(rest[..start].to_string()));
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            pieces.push(Piece::Text(rest[start..].to_string()));
            rest = "";
            break;
        };
        let (name, params) = after[..end].split_once(':').unwrap_or((&after[..end], ""));
        match name {
            "capturetime" | "now" => {
                let format = params.find("d\"").and_then(|at| {
                    let f = &params[at + 2..];
                    f.find('"').map(|close| &f[..close])
                });
                dotnet_date(format.unwrap_or("yyyy-MM-dd HH-mm-ss"), &mut pieces);
            }
            "YYYY" => pieces.push(Piece::Token("yyyy")),
            "MM" => pieces.push(Piece::Token("MM")),
            "DD" => pieces.push(Piece::Token("dd")),
            "hh" => pieces.push(Piece::Token("HH")),
            "mm" => pieces.push(Piece::Token("mm")),
            "ss" => pieces.push(Piece::Token("ss")),
            _ => pieces.push(Piece::Gone),
        }
        rest = &after[end + 1..];
    }
    pieces.push(Piece::Text(rest.to_string()));
    assemble(pieces)
}

/// Flameshot's name pattern, in strftime's terms (`%F_%H-%M`).
fn strftime_name(pattern: &str) -> Option<String> {
    let mut pieces = Vec::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            // Flameshot turns colons into dashes, as Windows doesn't allow them.
            pieces.push(Piece::Text(if c == ':' { "-".into() } else { c.to_string() }));
            continue;
        }
        let tokens: &[&'static str] = match chars.next() {
            Some('Y') => &["yyyy"],
            Some('m') => &["MM"],
            Some('d' | 'e') => &["dd"],
            Some('H' | 'I') => &["HH"],
            Some('M') => &["mm"],
            Some('S') => &["ss"],
            Some('F') => &["yyyy", "-", "MM", "-", "dd"],
            Some('T') => &["HH", "-", "mm", "-", "ss"],
            Some('R') => &["HH", "-", "mm"],
            Some('%') => {
                pieces.push(Piece::Text("%".into()));
                continue;
            }
            _ => {
                pieces.push(Piece::Gone);
                continue;
            }
        };
        for t in tokens {
            pieces.push(if *t == "-" { Piece::Text("-".into()) } else { Piece::Token(t) });
        }
    }
    assemble(pieces)
}

// --- Values -----------------------------------------------------------------------------------

/// `#rrggbb`, or with alpha: `#aarrggbb` when `alpha_first` (ShareX), `#rrggbbaa` otherwise
/// (Flameshot). A see-through colour doesn't come over.
fn hex_color(text: &str, alpha_first: bool) -> Option<Rgba> {
    let hex = text.trim().strip_prefix('#')?;
    if !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    let (r, g, b, a) = match hex.len() {
        6 => (byte(0)?, byte(2)?, byte(4)?, 255),
        8 if alpha_first => (byte(2)?, byte(4)?, byte(6)?, byte(0)?),
        8 => (byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        _ => return None,
    };
    (a > 0).then(|| Rgba::rgb(r, g, b))
}

/// A .NET colour: "242, 60, 60", "255, 242, 60, 60" (alpha first), or a name such as "Red".
fn dotnet_color(text: &str) -> Option<Rgba> {
    let text = text.trim();
    if text.contains(',') {
        let n: Vec<u8> = text.split(',').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
        return match n[..] {
            [r, g, b] => Some(Rgba::rgb(r, g, b)),
            [a, r, g, b] if a > 0 => Some(Rgba::rgb(r, g, b)),
            _ => None,
        };
    }
    let (r, g, b) = match text.to_ascii_lowercase().as_str() {
        "red" => (255, 0, 0),
        "darkred" => (139, 0, 0),
        "orange" => (255, 165, 0),
        "yellow" => (255, 255, 0),
        "lime" => (0, 255, 0),
        "green" => (0, 128, 0),
        "blue" => (0, 0, 255),
        "cyan" | "aqua" => (0, 255, 255),
        "magenta" | "fuchsia" => (255, 0, 255),
        "purple" => (128, 0, 128),
        "pink" => (255, 192, 203),
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "gray" | "grey" => (128, 128, 128),
        _ => return None,
    };
    Some(Rgba::rgb(r, g, b))
}

fn yes_no(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// A path with `%Name%` (ShareX) or `${Name}` (Greenshot) in it, filled in. `None` if a name
/// isn't known, or nothing's left.
fn expand(path: &str, open: &str, close: char, vars: Vars) -> Option<String> {
    let mut out = String::new();
    let mut rest = path;
    while let Some(start) = rest.find(open) {
        let after = &rest[start + open.len()..];
        let end = after.find(close)?;
        out.push_str(&rest[..start]);
        out.push_str(&vars(&after[..end])?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// An INI file as "section.key" (lower case) to value.
fn ini(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut section = String::new();
    for line in strip_bom(text).lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_ascii_lowercase();
        } else if let Some((key, value)) = line.split_once('=') {
            let value = value.trim();
            let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
            map.insert(format!("{section}.{}", key.trim().to_ascii_lowercase()), value.to_string());
        }
    }
    map
}

// --- The apps ---------------------------------------------------------------------------------

/// Where ShareX keeps its shortcuts, if it was told to keep them somewhere else, from
/// `ApplicationConfig.json`. A path, which may hold `%Name%` variables.
pub fn sharex_hotkeys_path(config: &str) -> Option<String> {
    let root: Value = serde_json::from_str(strip_bom(config)).ok()?;
    let path = root["CustomHotkeysConfigPath"].as_str()?.trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// A part of a ShareX task's settings: the region shortcut's own, if it says not to use the
/// defaults for it, otherwise the defaults.
fn sharex_part<'a>(task: Option<&'a Value>, defaults: &'a Value, name: &str, use_default: &str) -> &'a Value {
    match task {
        Some(task) if task[use_default].as_bool() == Some(false) => &task[name],
        _ => &defaults[name],
    }
}

/// ShareX, from `ApplicationConfig.json` and `HotkeysConfig.json`.
pub fn sharex(config: &str, hotkeys: Option<&str>, vars: Vars) -> Found {
    let mut f = Found::default();
    let root: Value = serde_json::from_str(strip_bom(config)).unwrap_or_default();
    let keys: Value = hotkeys.and_then(|t| serde_json::from_str(strip_bom(t)).ok()).unwrap_or_default();
    let mut region_task = None;
    for entry in keys["Hotkeys"].as_array().into_iter().flatten() {
        let task = &entry["TaskSettings"];
        let slot = match task["Job"].as_str().unwrap_or_default() {
            "RectangleRegion" | "RectangleTransparent" | "CustomRegion" => 0,
            "PrintScreen" | "ActiveMonitor" => 1,
            "OCR" => 2,
            "LastRegion" => 3,
            _ => continue,
        };
        let info = &entry["HotkeyInfo"];
        let Some(keys) = info["Hotkey"].as_str().and_then(|k| sharex_shortcut(k, info["Win"].as_bool().unwrap_or(false))) else { continue };
        if slot == 0 && region_task.is_none() {
            region_task = Some(task);
        }
        f.put(slot, keys);
    }
    let defaults = &root["DefaultTaskSettings"];
    let part = |name: &str, flag: &str| sharex_part(region_task, defaults, name, flag);

    if let Some(job) = part("AfterCaptureJob", "UseDefaultAfterCaptureJob").as_str() {
        let flags: Vec<&str> = job.split(',').map(str::trim).collect();
        f.enter_action = match (flags.contains(&"CopyImageToClipboard"), flags.contains(&"SaveImageToFile")) {
            (true, true) => Some(EnterAction::CopyAndSave),
            (true, false) => Some(EnterAction::Copy),
            (false, true) => Some(EnterAction::Save),
            (false, false) => None,
        };
    }
    let capture = part("CaptureSettings", "UseDefaultCaptureSettings");
    f.capture_cursor = capture["ShowCursor"].as_bool();
    // The editor's options moved in ShareX 22: RegionCaptureOptions, SurfaceOptions before.
    f.magnifier = capture["RegionCaptureOptions"]["ShowMagnifier"].as_bool().or(capture["SurfaceOptions"]["ShowMagnifier"].as_bool());
    f.notify = part("GeneralSettings", "UseDefaultGeneralSettings")["ShowToastNotificationAfterTaskCompleted"].as_bool();
    f.file_name = part("UploadSettings", "UseDefaultUploadSettings")["NameFormatPattern"].as_str().and_then(sharex_name);
    let editor = &part("ToolsSettings", "UseDefaultToolsSettings")["ImageEditorOptions"];
    let annotation = &capture["SurfaceOptions"]["AnnotationOptions"];
    f.color = editor["BorderColorHex"].as_str().and_then(|c| hex_color(c, true)).or_else(|| annotation["BorderColor"].as_str().and_then(dotnet_color));
    f.width = editor["Thickness"].as_f64().or(annotation["BorderSize"].as_f64()).map(|w| w as f32);
    if root["UseCustomScreenshotsPath"].as_bool() == Some(true) {
        f.save_dir = root["CustomScreenshotsPath"].as_str().and_then(|p| expand(p, "%", '%', vars));
    }
    f
}

/// Greenshot, from `Greenshot.ini`.
pub fn greenshot(text: &str, vars: Vars) -> Found {
    let mut f = Found::default();
    let ini = ini(text);
    let core = |key: &str| ini.get(&format!("core.{}", key.to_ascii_lowercase())).map(String::as_str);
    for (slot, key) in [(0, "RegionHotkey"), (1, "FullscreenHotkey"), (3, "LastregionHotkey")] {
        if let Some(keys) = core(key).and_then(greenshot_shortcut) {
            f.put(slot, keys);
        }
    }
    if let Some(list) = core("Destinations") {
        let to: Vec<&str> = list.split([',', '|']).map(str::trim).collect();
        f.enter_action = match (to.contains(&"Clipboard"), to.contains(&"FileNoDialog")) {
            (true, true) => Some(EnterAction::CopyAndSave),
            (true, false) => Some(EnterAction::Copy),
            (false, true) => Some(EnterAction::Save),
            (false, false) => None,
        };
    }
    f.capture_cursor = core("CaptureMousepointer").and_then(yes_no);
    f.magnifier = core("ZoomerEnabled").and_then(yes_no);
    f.notify = core("ShowTrayNotification").and_then(yes_no);
    f.save_dir = core("OutputFilePath").and_then(|p| expand(p, "${", '}', vars));
    f.file_name = core("OutputFileFilenamePattern").and_then(greenshot_name);
    // The rectangle's last colour and line: Prism's first tool is the rectangle too. Each is a
    // .NET type, a colon, then the value.
    let field = |name: &str| ini.get(&format!("editor.lastfieldvalue.rectanglecontainer.{name}")).and_then(|v| v.split_once(':')).map(|(_, v)| v.trim());
    f.color = field("line_color").and_then(dotnet_color);
    f.width = field("line_thickness").and_then(|w| w.parse::<f32>().ok()).filter(|w| *w > 0.0);
    f
}

/// Flameshot, from `flameshot.ini`.
pub fn flameshot(text: &str) -> Found {
    // On Windows, Print Screen always captures a region in Flameshot.
    let mut f = Found { print_screen: true, ..Found::default() };
    let ini = ini(text);
    let general = |key: &str| ini.get(&format!("general.{}", key.to_ascii_lowercase())).map(String::as_str);
    if let Some(keys) = ini.get("shortcuts.take_screenshot").and_then(|k| plus_shortcut(k)) {
        f.put(0, keys);
    }
    f.save_dir = general("savePath").map(|p| p.trim().replace('/', "\\")).filter(|p| !p.is_empty());
    f.file_name = general("filenamePattern").and_then(strftime_name);
    f.save_copies = general("saveAfterCopy").and_then(yes_no);
    f.notify = general("showDesktopNotification").and_then(yes_no);
    f.magnifier = general("showMagnifier").and_then(yes_no);
    f.color = general("drawColor").and_then(|c| hex_color(c, false));
    f.width = general("drawThickness").and_then(|w| w.trim().parse::<f32>().ok()).filter(|w| *w > 0.0);
    if let Some(list) = general("userColors") {
        let colors: Vec<Rgba> = list.split(',').filter_map(|c| hex_color(c, false)).collect();
        f.palette = (!colors.is_empty()).then_some(colors);
    }
    f
}

/// Lightshot, as it comes: Print Screen captures a region, and Shift+Print Screen captures the
/// whole screen. Lightshot keeps its settings in the registry in a form it doesn't document, so
/// its own defaults are what come over.
pub fn lightshot() -> Found {
    Found { print_screen: true, shortcuts: [None, Some("Shift+PrintScreen".into()), None, None], ..Found::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Stamp = Stamp { year: 2026, month: 11, day: 10, hour: 14, minute: 5, second: 9 };

    fn vars(name: &str) -> Option<String> {
        match name {
            "MyPictures" => Some(r"C:\Users\me\Pictures".into()),
            "MyDocuments" => Some(r"C:\Users\me\Documents".into()),
            "USERPROFILE" => Some(r"C:\Users\me".into()),
            _ => None,
        }
    }

    #[test]
    fn shortcuts_read_the_way_prism_writes_them() {
        assert_eq!(sharex_shortcut("PrintScreen, Control", false).as_deref(), Some("Ctrl+PrintScreen"));
        assert_eq!(sharex_shortcut("D1, Shift, Control", true).as_deref(), Some("Ctrl+Shift+Win+1"));
        assert_eq!(sharex_shortcut("Snapshot", false).as_deref(), Some("PrintScreen"));
        assert_eq!(sharex_shortcut("None", false), None);
        assert_eq!(sharex_shortcut("A", false), None, "a letter alone would get in the way of typing");
        assert_eq!(greenshot_shortcut("Alt + Ctrl + PrintScreen").as_deref(), Some("Ctrl+Alt+PrintScreen"));
        assert_eq!(greenshot_shortcut("Shift + PrintScreen").as_deref(), Some("Shift+PrintScreen"));
        assert_eq!(greenshot_shortcut("Win + F"), Some("Win+F".into()));
        assert_eq!(greenshot_shortcut("None"), None);
        assert_eq!(plus_shortcut("Meta+Shift+x").as_deref(), Some("Shift+Win+X"));
        assert_eq!(plus_shortcut("Print").as_deref(), Some("PrintScreen"));
        assert_eq!(plus_shortcut("Ctrl+NumPad1"), None);
        assert!(same_keys("alt + ctrl + s", "Ctrl+Alt+S"));
    }

    #[test]
    fn file_names_keep_what_prism_can_fill_in() {
        assert_eq!(sharex_name("%y-%mo-%d_%h-%mi-%s").as_deref(), Some("{date}_{HH}-{mm}-{ss}"));
        assert_eq!(sharex_name("ShareX_%pn_%y%mo%d_%width x%height").as_deref(), Some("ShareX_{yyyy}{MM}{dd}_{w} x{h}"));
        assert_eq!(sharex_name("%ra{10}"), None, "nothing left to tell the files apart");
        assert_eq!(sharex_name("%pn_%ra{10}"), None);
        assert_eq!(sharex_name("%mon %d").as_deref(), Some("{dd}"));
        assert_eq!(greenshot_name("${capturetime:d\"yyyy-MM-dd HH_mm_ss\"}-${title}").as_deref(), Some("{date} {HH}_{mm}_{ss}"));
        assert_eq!(greenshot_name("${title} ${capturetime}").as_deref(), Some("{date} {HH}-{mm}-{ss}"));
        assert_eq!(greenshot_name("shot-${YYYY}${MM}${DD}-${NUM}").as_deref(), Some("shot-{yyyy}{MM}{dd}"));
        assert_eq!(greenshot_name("${capturetime:d\"yyyy 'at' HH.mm\"}").as_deref(), Some("{yyyy} at {HH}.{mm}"));
        assert_eq!(strftime_name("%F_%H-%M").as_deref(), Some("{date}_{HH}-{mm}"));
        assert_eq!(strftime_name("%T %a").as_deref(), Some("{HH}-{mm}-{ss}"));
        assert_eq!(strftime_name("shot").as_deref(), None);
    }

    #[test]
    fn colors_and_paths() {
        assert_eq!(hex_color("#FFF23C3C", true), Some(Rgba::rgb(0xF2, 0x3C, 0x3C)));
        assert_eq!(hex_color("#f23c3cff", false), Some(Rgba::rgb(0xF2, 0x3C, 0x3C)));
        assert_eq!(hex_color("#00F23C3C", true), None, "see-through");
        assert_eq!(dotnet_color("242, 60, 60"), Some(Rgba::rgb(242, 60, 60)));
        assert_eq!(dotnet_color("Red"), Some(Rgba::rgb(255, 0, 0)));
        assert_eq!(dotnet_color("Transparent"), None);
        assert_eq!(expand(r"%MyDocuments%\Shots", "%", '%', &vars).as_deref(), Some(r"C:\Users\me\Documents\Shots"));
        assert_eq!(expand(r"${MyPictures}\Greenshots", "${", '}', &vars).as_deref(), Some(r"C:\Users\me\Pictures\Greenshots"));
        assert_eq!(expand(r"%Nope%\x", "%", '%', &vars), None);
    }

    const SHAREX_CONFIG: &str = r#"{
      "DefaultTaskSettings": {
        "AfterCaptureJob": "CopyImageToClipboard, SaveImageToFile, AnnotateImage",
        "GeneralSettings": { "PlaySoundAfterCapture": true, "ShowToastNotificationAfterTaskCompleted": false },
        "CaptureSettings": { "ShowCursor": true,
          "SurfaceOptions": { "ShowMagnifier": false,
            "AnnotationOptions": { "BorderColor": "59, 130, 246", "BorderSize": 5 } } },
        "UploadSettings": { "NameFormatPattern": "%y-%mo-%d_%h-%mi-%s" }
      },
      "UseCustomScreenshotsPath": true, "CustomScreenshotsPath": "%MyDocuments%\\Shots",
      "SaveImageSubFolderPattern": "%y-%mo",
      "ApplicationVersion": "21.0.0"
    }"#;
    const SHAREX_HOTKEYS: &str = r#"{
      "Hotkeys": [
        { "HotkeyInfo": { "Hotkey": "PrintScreen, Control", "Win": false }, "TaskSettings": { "Job": "RectangleRegion", "UseDefaultAfterCaptureJob": true } },
        { "HotkeyInfo": { "Hotkey": "PrintScreen", "Win": false }, "TaskSettings": { "Job": "PrintScreen" } },
        { "HotkeyInfo": { "Hotkey": "PrintScreen, Alt", "Win": false }, "TaskSettings": { "Job": "ActiveWindow" } },
        { "HotkeyInfo": { "Hotkey": "None", "Win": false }, "TaskSettings": { "Job": "LastRegion" } },
        { "HotkeyInfo": { "Hotkey": "T, Shift, Control", "Win": false }, "TaskSettings": { "Job": "OCR" } }
      ],
      "ApplicationVersion": "21.0.0"
    }"#;

    #[test]
    fn reads_sharex() {
        let f = sharex(SHAREX_CONFIG, Some(SHAREX_HOTKEYS), &vars);
        assert!(!f.print_screen);
        assert_eq!(f.shortcuts, [Some("Ctrl+PrintScreen".into()), Some("PrintScreen".into()), Some("Ctrl+Shift+T".into()), None]);
        assert_eq!(f.enter_action, Some(EnterAction::CopyAndSave));
        assert_eq!((f.capture_cursor, f.magnifier, f.notify), (Some(true), Some(false), Some(false)));
        assert_eq!(f.file_name.as_deref(), Some("{date}_{HH}-{mm}-{ss}"));
        assert_eq!(f.save_dir.as_deref(), Some(r"C:\Users\me\Documents\Shots"));
        assert_eq!((f.color, f.width), (Some(Rgba::rgb(59, 130, 246)), Some(5.0)));
        // ShareX 22 keeps the editor's options elsewhere.
        let v22 = r##"{"DefaultTaskSettings": {"CaptureSettings": {"RegionCaptureOptions": {"ShowMagnifier": true}},
            "ToolsSettings": {"ImageEditorOptions": {"BorderColorHex": "#FFF23C3C", "Thickness": 4}}}}"##;
        let f = sharex(v22, None, &vars);
        assert_eq!((f.magnifier, f.color, f.width), (Some(true), Some(Rgba::rgb(0xF2, 0x3C, 0x3C)), Some(4.0)));
        assert_eq!(sharex("not json", None, &vars), Found::default());
        assert_eq!(sharex_hotkeys_path(r#"{"CustomHotkeysConfigPath": " D:\\keys.json "}"#).as_deref(), Some(r"D:\keys.json"));
    }

    #[test]
    fn sharex_uses_the_region_shortcuts_own_settings_when_it_has_them() {
        let hotkeys = r#"{"Hotkeys": [{ "HotkeyInfo": { "Hotkey": "A, Control, Alt" },
            "TaskSettings": { "Job": "RectangleRegion", "UseDefaultAfterCaptureJob": false, "AfterCaptureJob": "SaveImageToFile" } }]}"#;
        let f = sharex(SHAREX_CONFIG, Some(hotkeys), &vars);
        assert_eq!(f.enter_action, Some(EnterAction::Save));
        assert_eq!(f.shortcuts[0].as_deref(), Some("Ctrl+Alt+A"));
    }

    const GREENSHOT: &str = "\u{feff}[Core]\r\n; Hotkey for starting the region capture\r\nRegionHotkey=PrintScreen\r\nWindowHotkey=Alt + PrintScreen\r\nFullscreenHotkey=Ctrl + PrintScreen\r\nLastregionHotkey=Shift + PrintScreen\r\nDestinations=FileNoDialog,Clipboard\r\nCaptureMousepointer=False\r\nShowTrayNotification=True\r\nOutputFilePath=${MyPictures}\\Greenshots\r\nOutputFileFilenamePattern=${capturetime:d\"yyyy-MM-dd HH_mm_ss\"}-${title}\r\nZoomerEnabled=True\r\n[Editor]\r\nRecentColors=0, 0, 0|255, 255, 255|Transparent\r\nLastFieldValue.RectangleContainer.LINE_THICKNESS=System.Int32,mscorlib, Version=4.0.0.0, Culture=neutral, PublicKeyToken=b77a5c561934e089:3\r\nLastFieldValue.RectangleContainer.LINE_COLOR=System.Drawing.Color,System.Drawing, Version=4.0.0.0, Culture=neutral, PublicKeyToken=b03f5f7f11d50a3a:Red\r\n";

    #[test]
    fn reads_greenshot() {
        let f = greenshot(GREENSHOT, &vars);
        assert!(f.print_screen);
        assert_eq!(f.shortcuts, [None, Some("Ctrl+PrintScreen".into()), None, Some("Shift+PrintScreen".into())]);
        assert_eq!(f.enter_action, Some(EnterAction::CopyAndSave));
        assert_eq!((f.capture_cursor, f.magnifier, f.notify), (Some(false), Some(true), Some(true)));
        assert_eq!(f.save_dir.as_deref(), Some(r"C:\Users\me\Pictures\Greenshots"));
        assert_eq!(f.file_name.as_deref(), Some("{date} {HH}_{mm}_{ss}"));
        assert_eq!((f.color, f.width), (Some(Rgba::rgb(255, 0, 0)), Some(3.0)));
        let picker = greenshot("[Core]\nDestinations=Picker\nOutputFilePath=\n", &vars);
        assert_eq!((picker.enter_action, picker.save_dir), (None, None));
    }

    const FLAMESHOT: &str = "[General]\nsavePath=C:/Users/me/Pictures/Flameshot\nsavePathFixed=true\nsaveAfterCopy=true\nfilenamePattern=%F_%H-%M\ndrawColor=#3b82f6\ndrawThickness=6\nuserColors=picker, #800000, #ff0000, #ffff00\nshowDesktopNotification=false\nshowMagnifier=true\n\n[Shortcuts]\nTAKE_SCREENSHOT=Ctrl+Shift+x\n";

    #[test]
    fn reads_flameshot() {
        let f = flameshot(FLAMESHOT);
        assert!(f.print_screen);
        assert_eq!(f.shortcuts[0].as_deref(), Some("Ctrl+Shift+X"));
        assert_eq!(f.save_dir.as_deref(), Some(r"C:\Users\me\Pictures\Flameshot"));
        assert_eq!(f.file_name.as_deref(), Some("{date}_{HH}-{mm}"));
        assert_eq!((f.save_copies, f.notify, f.magnifier), (Some(true), Some(false), Some(true)));
        assert_eq!((f.color, f.width), (Some(Rgba::rgb(0x3B, 0x82, 0xF6)), Some(6.0)));
        assert_eq!(f.palette, Some(vec![Rgba::rgb(0x80, 0, 0), Rgba::rgb(0xFF, 0, 0), Rgba::rgb(0xFF, 0xFF, 0)]));
        // Even an empty file means Print Screen captures a region.
        assert!(flameshot("").print_screen);
    }

    #[test]
    fn lightshot_brings_its_default_keys() {
        let mut s = Settings::default();
        let said = lightshot().apply(&mut s, T);
        assert!(s.replace_snipping);
        // Prism already captures the screen with Shift+Print Screen.
        assert_eq!(said, ["Print Screen captures a region"]);
        assert_eq!(s.hotkeys.screen, "Shift+PrintScreen");
    }

    #[test]
    fn applying_greenshot_says_what_changed() {
        let f = greenshot(GREENSHOT, &vars);
        let mut s = Settings::default();
        let said = f.apply(&mut s, T);
        assert!(s.replace_snipping);
        assert_eq!(s.hotkeys.screen, "Ctrl+PrintScreen");
        assert_eq!(s.hotkeys.last, "Shift+PrintScreen");
        // Prism's copy-text shortcut was Ctrl+Print Screen, which now captures the screen.
        assert_eq!(s.hotkeys.text, "");
        assert_eq!(s.hotkeys.region, Hotkeys::default().region, "kept, as Print Screen does it now");
        assert_eq!(s.enter_action, EnterAction::CopyAndSave);
        assert_eq!(s.style.color, Rgba::rgb(255, 0, 0));
        assert_eq!(s.style.width, 3.0);
        assert_eq!(
            sentence(&said),
            "Print Screen captures a region, Ctrl+Print Screen captures the screen, Shift+Print Screen captures the last region again, \
             copying text loses its shortcut, captures are saved in C:\\Users\\me\\Pictures\\Greenshots, \
             files are named like \u{201C}2026-11-10 14_05_09\u{201D}, finishing copies and saves, \
             you draw in its pen colour and the pen size is 3."
        );
        // Once it's in, there's nothing left to bring over.
        assert!(f.changes(&s, T).is_empty());
    }

    #[test]
    fn shortcuts_prism_cant_use_stay_as_they_are() {
        // Print Screen alone for the whole screen: in Prism it's only ever for a region.
        let f = Found { shortcuts: [None, Some("PrintScreen".into()), None, None], ..Found::default() };
        let mut s = Settings::default();
        assert!(f.apply(&mut s, T).is_empty());
        assert_eq!(s, Settings::default());
        // The same keys twice: the first gets them.
        let f = Found { shortcuts: [Some("Ctrl+Alt+A".into()), Some("Ctrl+Alt+A".into()), None, None], ..Found::default() };
        let mut s = Settings::default();
        f.apply(&mut s, T);
        assert_eq!((s.hotkeys.region.as_str(), s.hotkeys.screen.as_str()), ("Ctrl+Alt+A", Hotkeys::default().screen.as_str()));
        // Win+Shift+S belongs to Replace Snipping Tool once it's on.
        let f = Found { print_screen: true, shortcuts: [None, Some("Win+Shift+S".into()), None, None], ..Found::default() };
        let mut s = Settings::default();
        f.apply(&mut s, T);
        assert_eq!(s.hotkeys.screen, Hotkeys::default().screen);
    }

    #[test]
    fn undo_puts_back_only_what_was_brought_over() {
        let before = Settings { last_region: Some([1, 2, 30, 40]), ..Settings::default() };
        let mut s = before.clone();
        flameshot(FLAMESHOT).apply(&mut s, T);
        assert_ne!(s, before);
        s.magnifier = false;
        s.last_region = Some([5, 5, 50, 50]);
        undo(&before, &mut s);
        assert_eq!(s, Settings { last_region: Some([5, 5, 50, 50]), ..before });
    }

    #[test]
    fn widths_snap_to_prisms_sizes() {
        assert_eq!(nearest_width(5.0), 4.0);
        assert_eq!(nearest_width(8.0), 9.0);
        assert_eq!(nearest_width(40.0), 14.0);
        assert_eq!(sentence(&["a".into()]), "A.");
        assert_eq!(sentence(&["a".into(), "b".into()]), "A and b.");
        assert_eq!(sentence(&[]), "");
    }
}
