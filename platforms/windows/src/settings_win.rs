//! The settings window, in the look of Spotlight's: one page that scrolls under a header with
//! tabs that follow it, sections of glass cards, rows that light up under the pointer, and
//! controls that move. Everything is drawn here with tiny-skia, so it's sharp at any scale and
//! can animate. Only the two text boxes are Windows' own, and only while one is being typed in.
//!
//! Changes save as they're made, through `app::update_settings`, which puts each one into
//! effect straight away. Text boxes save a moment after typing stops.
//!
//! Shortcuts are recorded by pressing them. While a shortcut field listens, Prism lets go of its
//! own shortcuts (`app::pause_shortcuts`), so pressing Print Screen there records it rather than
//! starting a capture.
//!
//! Nothing animates, and no timer runs, unless something on screen is moving.

use crate::app::{self, Applied};
use crate::hotkey::{self, Held};
use crate::text::{GdiText, ui_face};
use crate::ui::{self, Btn, IconExtra};
use crate::util::wide;
use prism_core::doc::{Tool, ToolKeys, WIDTHS};
use prism_core::icon::rounded_rect;
use prism_core::render::{blend_mask, paint_color};
use prism_core::settings::{EnterAction, FinishKey, Hotkeys, Loaded, PopupPosition, QuickFinish, Settings};
use prism_core::{IRect, Rgba, naming};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;
use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, PixmapMut, Stroke, Transform};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection,
    CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint, FF_DONTCARE, GdiFlush,
    GetMonitorInfoW, GetSysColor, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, OUT_TT_PRECIS, PAINTSTRUCT, SRCCOPY, ScreenToClient, SelectObject, SetBkColor, SetTextColor, SYS_COLOR_INDEX,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::Controls::{EM_CHARFROMPOS, EM_REPLACESEL, EM_SETCUEBANNER, EM_SETMARGINS, EM_SETSEL, MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForSystem, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_BACK, VK_CONTROL, VK_DELETE,
    VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SNAPSHOT,
    VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

pub const PAGE_GENERAL: usize = 0;
const TABS: [&str; 4] = ["General", "Shortcuts", "Capturing", "Saving"];
/// The tabs' names on the command line, for the developer preview.
const TAB_KEYS: [&str; 4] = ["general", "shortcuts", "capturing", "saving"];

// Sizes, in pixels at 100%.
const WIDTH: f32 = 860.0;
const HEIGHT: f32 = 680.0;
const TITLE_H: f32 = 66.0;
const HEADER_H: f32 = 114.0;
const MARGIN: f32 = 28.0;
const CAPTION_W: f32 = 46.0;
const CAPTION_H: f32 = 34.0;
const TAB_W: f32 = 98.0;
const TAB_H: f32 = 30.0;
const SEG_INSET: f32 = 3.0;
const ROW_PAD: f32 = 14.0;
const CARD_PAD: f32 = 5.0;
const SECTION_GAP: f32 = 28.0;
const CONTROL_H: f32 = 32.0;
const TOOL_COLUMNS: i32 = 3;

const ANIM_TIMER: usize = 1;
const TEXT_TIMER: usize = 2;
const SAVED_TIMER: usize = 3;
const BAR_TIMER: usize = 4;
const FRAME_MS: u32 = 15;
const TEXT_SAVE_DELAY_MS: u32 = 500;
const SAVED_MS: u32 = 1800;
const BAR_IDLE_MS: u32 = 900;

const SHORTCUTS: [(&str, &str); 3] = [
    ("Capture a region", "Drag to select, or click a window."),
    ("Capture the screen", "The whole screen under the pointer, straight away."),
    ("Copy text from the screen", "Select a region and the text in it is copied."),
];
const RECORDING_NOTE: &str = "Press the keys you want, or Esc to leave it as it was.";
const SHORTCUTS_NOTE: &str = "Click a shortcut and press new keys. While it listens, Backspace puts back the default and Delete turns it off.";
const TOOLS_NOTE: &str = "Click a letter, then press another. Giving a tool a letter another tool has swaps them. C, F and O are kept for the colour picker, the whole screen and copying text.";
const TOOL_LISTENING: &str = "Press a letter for this tool. Esc cancels and Backspace puts back its default.";
const SNIPPING_ON: &str = "These capture a region with Prism instead of opening Snipping Tool.";
const SNIPPING_OFF: &str = "Snipping Tool has these. Turn this on and they capture a region with Prism instead.";
const SNIPPING_FAILED: &str = "Couldn't take Win+Shift+S from Snipping Tool, so only Print Screen opens Prism. Try turning this off and on again.";
/// The keys Replace Snipping Tool brings, as their key caps show them.
const SNIPPING_KEYS: [&str; 2] = ["PrintScreen", "Win+Shift+S"];
const QUICK: [(&str, QuickFinish); 5] =
    [("Off", QuickFinish::Off), ("Shift", QuickFinish::Shift), ("Ctrl", QuickFinish::Ctrl), ("Alt", QuickFinish::Alt), ("Always", QuickFinish::Always)];
const FINISH_KEYS: [(&str, FinishKey); 3] = [("Enter", FinishKey::Enter), ("Esc", FinishKey::Escape), ("Both", FinishKey::Both)];
const ENTER: [(&str, EnterAction); 3] = [("Copy", EnterAction::Copy), ("Save", EnterAction::Save), ("Both", EnterAction::CopyAndSave)];
const POPUP_POSITIONS: [(&str, PopupPosition); 6] = [
    ("Top left", PopupPosition::TopLeft),
    ("Top centre", PopupPosition::TopCenter),
    ("Top right", PopupPosition::TopRight),
    ("Bottom left", PopupPosition::BottomLeft),
    ("Bottom centre", PopupPosition::BottomCenter),
    ("Bottom right", PopupPosition::BottomRight),
];
const CHIPS: [(&str, &str); 3] = [("Date", "{date}"), ("Time", "{time}"), ("Size", "{w}x{h}")];

fn shortcut(keys: &Hotkeys, slot: usize) -> &str {
    match slot {
        0 => &keys.region,
        1 => &keys.screen,
        _ => &keys.text,
    }
}

fn set_shortcut(keys: &mut Hotkeys, slot: usize, text: &str) {
    let field = match slot {
        0 => &mut keys.region,
        1 => &mut keys.screen,
        _ => &mut keys.text,
    };
    *field = text.to_string();
}

fn default_shortcut(slot: usize) -> String {
    shortcut(&Hotkeys::default(), slot).to_string()
}

/// Whether a shortcut is as it comes.
fn is_default(s: &State, slot: usize) -> bool {
    hotkey::same(shortcut(&s.settings.hotkeys, slot), &default_shortcut(slot))
}

/// The colours a capture can start with, after "Last": the eight on the toolbar, and the chosen
/// one if it isn't among them.
fn color_choices(settings: &Settings) -> Vec<Rgba> {
    let mut colors: Vec<Rgba> = settings.palette.iter().take(8).copied().collect();
    if let Some(c) = settings.start_color.filter(|c| !colors.contains(c)) {
        colors.push(c);
    }
    colors
}

/// The sizes a capture can start with, after "Last": the toolbar's steps, and the chosen one if
/// it isn't among them.
fn width_choices(settings: &Settings) -> Vec<f32> {
    let mut widths = WIDTHS.to_vec();
    if let Some(w) = settings.start_width.filter(|w| !widths.iter().any(|x| (x - w).abs() < 0.01)) {
        widths.push(w);
        widths.sort_by(f32::total_cmp);
    }
    widths
}

fn color_name(c: Rgba) -> String {
    const NAMES: [&str; 8] = ["red", "orange", "yellow", "green", "blue", "purple", "black", "white"];
    match Rgba::DEFAULT_PALETTE.iter().position(|x| *x == c) {
        Some(i) => NAMES[i].to_string(),
        None => c.to_hex(),
    }
}

/// The tool a "Pick up a tool" choice means: the first choice, Select, is "none".
fn auto_tool_of(choice: usize) -> Option<Tool> {
    Tool::ALL.get(choice).copied().filter(|t| *t != Tool::Select)
}

fn auto_tool_choice(tool: Option<Tool>) -> usize {
    tool.and_then(|t| Tool::ALL.iter().position(|x| *x == t)).unwrap_or(0)
}

// --- Colours ----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Palette {
    dark: bool,
    /// Windows' high contrast: lines are drawn solid, in its own colours.
    high_contrast: bool,
    bg: Rgba,
    /// What see-through fills and lines are made of: white on dark, black on light.
    ink: Rgba,
    text: Rgba,
    secondary: Rgba,
    subtle: Rgba,
    muted: Rgba,
    accent: Rgba,
    on_accent: Rgba,
    warning: Rgba,
}

const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    Rgba { r, g, b, a: 255 }
}

/// Prism's violet, from its icon, a step lighter on dark so it reads.
const DARK: Palette = Palette {
    dark: true,
    high_contrast: false,
    bg: rgb(0x12, 0x12, 0x14),
    ink: rgb(0xFF, 0xFF, 0xFF),
    text: rgb(0xF5, 0xF5, 0xF7),
    secondary: rgb(0xCD, 0xCD, 0xD2),
    subtle: rgb(0xA0, 0xA0, 0xA8),
    muted: rgb(0x76, 0x76, 0x7E),
    accent: rgb(0x8B, 0x5C, 0xF6),
    on_accent: rgb(0xFF, 0xFF, 0xFF),
    warning: rgb(0xF5, 0xA5, 0x24),
};

const LIGHT: Palette = Palette {
    dark: false,
    high_contrast: false,
    bg: rgb(0xF6, 0xF6, 0xF8),
    ink: rgb(0x00, 0x00, 0x00),
    text: rgb(0x18, 0x18, 0x1B),
    secondary: rgb(0x3F, 0x3F, 0x46),
    subtle: rgb(0x5E, 0x5E, 0x68),
    muted: rgb(0x8A, 0x8A, 0x93),
    accent: rgb(0x7C, 0x3A, 0xED),
    on_accent: rgb(0xFF, 0xFF, 0xFF),
    warning: rgb(0xB4, 0x53, 0x09),
};

impl Palette {
    fn current() -> Palette {
        if is_high_contrast() {
            let c = |index: i32| {
                let v = unsafe { GetSysColor(SYS_COLOR_INDEX(index)) };
                rgb(v as u8, (v >> 8) as u8, (v >> 16) as u8)
            };
            // Window, window text, grey text, highlight, highlight text.
            let (bg, text, grey, hi, hi_text) = (c(5), c(8), c(17), c(13), c(14));
            return Palette {
                dark: luminance(bg) < 0.5,
                high_contrast: true,
                bg,
                ink: text,
                text,
                secondary: text,
                subtle: text,
                muted: grey,
                accent: hi,
                on_accent: hi_text,
                warning: text,
            };
        }
        if system_uses_dark_apps() { DARK } else { LIGHT }
    }

    /// The ink at an opacity: the stuff glass and quiet fills are made of.
    fn ink(&self, opacity: f32) -> Rgba {
        alpha(self.ink, opacity)
    }

    /// A line: see-through normally, solid in high contrast so it shows.
    fn line(&self, opacity: f32) -> Rgba {
        if self.high_contrast { self.text } else { self.ink(opacity) }
    }

    fn tone(&self, tone: Tone) -> Rgba {
        match tone {
            Tone::Subtle => self.subtle,
            Tone::Muted => self.muted,
            Tone::Accent => self.accent,
            Tone::Warning => self.warning,
        }
    }
}

fn alpha(c: Rgba, opacity: f32) -> Rgba {
    Rgba { a: (c.a as f32 * opacity.clamp(0.0, 1.0)).round() as u8, ..c }
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgba { r: ch(a.r, b.r), g: ch(a.g, b.g), b: ch(a.b, b.b), a: ch(a.a, b.a) }
}

/// `top` laid over an opaque `base`.
fn over(base: Rgba, top: Rgba) -> Rgba {
    let t = top.a as f32 / 255.0;
    Rgba { a: 255, ..mix(base, Rgba { a: 255, ..top }, t) }
}

fn luminance(c: Rgba) -> f32 {
    (0.2126 * c.r as f32 + 0.7152 * c.g as f32 + 0.0722 * c.b as f32) / 255.0
}

fn colorref(c: Rgba) -> COLORREF {
    COLORREF(c.r as u32 | (c.g as u32) << 8 | (c.b as u32) << 16)
}

fn is_high_contrast() -> bool {
    let mut info = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    unsafe {
        SystemParametersInfoW(SPI_GETHIGHCONTRAST, info.cbSize, Some(&mut info as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).is_ok()
            && info.dwFlags.contains(HCF_HIGHCONTRASTON)
    }
}

fn system_uses_dark_apps() -> bool {
    let mut value = 1u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut _ as *mut _),
            Some(&mut size),
        )
    };
    status.is_ok() && value == 0
}

/// Whether Windows' "Animation effects" is on.
fn motion_allowed() -> bool {
    let mut on = BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    on.as_bool()
}

// --- Animation --------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Key {
    /// A row or control lit under the pointer.
    Hover(Ctl),
    /// One choice of several lit under the pointer.
    HoverPart(Ctl, usize),
    /// A switch's knob, or the pill of a set of choices, at its place.
    Slide(Ctl),
    Pressed(Ctl),
    Divider,
    Saved,
    Bar,
}

#[derive(Clone, Copy)]
enum Motion {
    /// Straight there, at so much per second up and down: hover lights on fast and lets go slower.
    Fade { up: f32, down: f32 },
    /// A spring that overshoots a little and settles.
    Spring,
}

struct Anim {
    value: f32,
    velocity: f32,
    target: f32,
    motion: Motion,
}

const HOVER: Motion = Motion::Fade { up: 1.0 / 0.03, down: 1.0 / 0.28 };
const QUICK_FADE: Motion = Motion::Fade { up: 1.0 / 0.09, down: 1.0 / 0.09 };
const SLOW_FADE: Motion = Motion::Fade { up: 1.0 / 0.18, down: 1.0 / 0.3 };

#[derive(Default)]
struct Anims {
    map: HashMap<Key, Anim>,
    last: Option<Instant>,
    /// Windows' animation effects are off: everything jumps.
    still: bool,
}

impl Anims {
    /// The value for `key` as it heads for `target`. New keys start there, so nothing animates
    /// the first time it's drawn.
    fn follow(&mut self, key: Key, target: f32, motion: Motion) -> f32 {
        let still = self.still;
        let anim = self.map.entry(key).or_insert(Anim { value: target, velocity: 0.0, target, motion });
        anim.target = target;
        anim.motion = motion;
        if still {
            anim.value = target;
            anim.velocity = 0.0;
        }
        anim.value
    }

    fn value(&self, key: Key) -> f32 {
        self.map.get(&key).map(|a| a.value).unwrap_or(0.0)
    }

    /// Move everything on by the time since the last frame. True while anything still moves.
    fn tick(&mut self) -> bool {
        let now = Instant::now();
        let dt = self.last.map(|t| (now - t).as_secs_f32()).unwrap_or(0.0).min(0.05);
        self.last = Some(now);
        let mut moving = false;
        for anim in self.map.values_mut() {
            let gap = anim.target - anim.value;
            if gap.abs() < 0.001 && anim.velocity.abs() < 0.01 {
                anim.value = anim.target;
                anim.velocity = 0.0;
                continue;
            }
            match anim.motion {
                Motion::Fade { up, down } => {
                    let step = if gap > 0.0 { up } else { down } * dt;
                    anim.value = if gap.abs() <= step { anim.target } else { anim.value + step * gap.signum() };
                }
                Motion::Spring => {
                    // Stiff, and damped just under enough: a small overshoot, like ease-out-back.
                    let (stiffness, damping) = (520.0, 34.0);
                    anim.velocity += (stiffness * gap - damping * anim.velocity) * dt;
                    anim.value += anim.velocity * dt;
                }
            }
            moving = true;
        }
        moving
    }

    /// Whether anything is still on its way: drawing sets new targets after `tick`.
    fn busy(&self) -> bool {
        self.map.values().any(|a| (a.target - a.value).abs() >= 0.001 || a.velocity.abs() >= 0.01)
    }

    fn settle(&mut self) {
        self.last = None;
    }
}

// --- What's on the page -----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Ctl {
    Tab,
    Minimize,
    Close,
    Autostart,
    Snipping,
    Recorder(usize),
    ToolKey(usize),
    ResetToolKeys,
    Magnifier,
    Cursor,
    QuickFinish,
    AutoTool,
    StartColor,
    StartWidth,
    FinishKey,
    EnterAction,
    SaveCopies,
    Notify,
    PopupPosition,
    Folder,
    Browse,
    OpenFolder,
    FileName,
    Chip(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Folder,
    FileName,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tone {
    Subtle,
    Muted,
    Accent,
    Warning,
}

#[derive(Clone)]
enum Seg {
    Text(&'static str),
    Icon(Tool),
    /// A colour, as a dot of it.
    Swatch(Rgba),
    /// A size, as a dot that big.
    Dot(f32),
}

#[derive(Clone)]
enum Kind {
    Toggle(bool),
    Segments { segs: Vec<Seg>, selected: usize, seg_w: i32 },
    Button { label: &'static str, ghost: bool },
    Recorder(usize),
    ToolKey(usize),
    Field(Field),
    Picker(usize),
}

#[derive(Clone)]
struct Control {
    ctl: Ctl,
    /// In page coordinates: before scrolling, from the top of the scrolling area.
    rect: IRect,
    kind: Kind,
}

struct Row {
    rect: IRect,
    label: String,
    label_x: i32,
    desc: Vec<String>,
    tone: Tone,
    /// What the row lights for and, for a switch, what a click anywhere on it toggles.
    ctl: Option<Ctl>,
    toggles: bool,
    icon: Option<Tool>,
    /// Key caps drawn before the control, ending at `caps_right`: groups such as "Win+Shift+S",
    /// with "or" between them. Faint while `caps_on` is false.
    caps: Vec<String>,
    caps_right: i32,
    caps_on: bool,
}

struct Note {
    x: i32,
    y: i32,
    lines: Vec<String>,
    tone: Tone,
}

struct Heading {
    text: &'static str,
    x: i32,
    y: i32,
}

#[derive(Default)]
struct Scene {
    height: i32,
    cards: Vec<IRect>,
    headings: Vec<Heading>,
    rows: Vec<Row>,
    controls: Vec<Control>,
    notes: Vec<Note>,
    /// Where each tab's sections start, in page coordinates.
    tab_tops: [i32; 4],
}

// --- State ------------------------------------------------------------------------------------

struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    w: i32,
    h: i32,
}

impl Canvas {
    fn new(w: i32, h: i32) -> Option<Canvas> {
        if w <= 0 || h <= 0 {
            return None;
        }
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
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
            let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return None;
            };
            let old = SelectObject(dc, bitmap.into());
            Some(Canvas { dc, bitmap, old, bits: bits as *mut u8, w, h })
        }
    }

    fn pixmap(&mut self) -> PixmapMut<'_> {
        let slice = unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize) };
        PixmapMut::from_bytes(slice, self.w as u32, self.h as u32).expect("canvas")
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

struct State {
    hwnd: HWND,
    scale: f32,
    palette: Palette,
    font: GdiText,
    canvas: Option<Canvas>,
    settings: Settings,
    /// Read from the registry once, as it isn't part of the settings file.
    autostart: bool,
    snipping_failed: bool,
    scene: Scene,
    scene_stale: bool,
    anims: Anims,
    scroll: f32,
    scroll_to: f32,
    /// A tab clicked stays the current one until the page is scrolled some other way, as the
    /// last section may be too short to reach the top.
    pinned_tab: Option<usize>,
    hover: Option<(Ctl, usize)>,
    hover_row: Option<Ctl>,
    pressed: Option<(Ctl, usize)>,
    thumb_grab: Option<i32>,
    near_bar: bool,
    tracking_leave: bool,
    focus: Option<Ctl>,
    /// The focus was moved with the keyboard, so it gets a ring.
    focus_ring: bool,
    recording: Option<usize>,
    /// Modifiers held while listening, as "Ctrl+Alt".
    held: String,
    /// The keys that just finished a recording are still down: their typing and release are ignored.
    swallow_keys: bool,
    slot_notes: [Option<String>; 3],
    /// Why Print Screen couldn't be taken, when Replace Snipping Tool is on.
    print_screen_problem: Option<String>,
    tool_listening: Option<usize>,
    tool_note: Option<String>,
    saved: Option<(String, bool)>,
    /// The "Saved" pill is showing; its text stays while it fades out.
    saved_on: bool,
    editing: Option<Field>,
    edit: HWND,
    edit_font: HFONT,
    edit_brush: HBRUSH,
    preview: bool,
}

thread_local! {
    static CURRENT: Cell<Option<HWND>> = const { Cell::new(None) };
}

fn state(hwnd: HWND) -> Option<&'static mut State> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut State;
    unsafe { pointer.as_mut() }
}

impl State {
    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    fn size(&self) -> (i32, i32) {
        self.canvas.as_ref().map(|c| (c.w, c.h)).unwrap_or((self.px(WIDTH), self.px(HEIGHT)))
    }

    fn header_h(&self) -> i32 {
        self.px(HEADER_H)
    }

    fn view_h(&self) -> i32 {
        self.size().1 - self.header_h()
    }

    fn max_scroll(&self) -> f32 {
        (self.scene.height - self.view_h()).max(0) as f32
    }

    /// A page rectangle where it is in the window now.
    fn on_screen(&self, r: IRect) -> IRect {
        r.offset(0, self.header_h() - self.scroll.round() as i32)
    }

    fn invalidate(&self) {
        if !self.hwnd.is_invalid() {
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }

    /// Something changed what's on the page.
    fn restage(&mut self) {
        self.scene_stale = true;
        self.invalidate();
    }

    fn control(&self, ctl: Ctl) -> Option<&Control> {
        self.scene.controls.iter().find(|c| c.ctl == ctl)
    }
}

// --- Text -------------------------------------------------------------------------------------

/// Font sizes and weights, in pixels at 100%.
#[derive(Clone, Copy)]
struct Font {
    px: f32,
    weight: u32,
}

const TITLE: Font = Font { px: 26.0, weight: 700 };
const HEADING: Font = Font { px: 16.0, weight: 700 };
const LABEL: Font = Font { px: 14.0, weight: 500 };
const BODY: Font = Font { px: 13.0, weight: 400 };
const SMALL: Font = Font { px: 12.0, weight: 400 };
const STRONG: Font = Font { px: 13.0, weight: 600 };
const CAP: Font = Font { px: 12.0, weight: 600 };

impl State {
    fn measure(&self, text: &str, font: Font) -> (i32, i32) {
        self.font.measure_weight(text, font.px * self.scale, font.weight)
    }

    fn line_h(&self, font: Font) -> i32 {
        self.measure("Ag", font).1
    }

    fn draw_text(&self, pm: &mut PixmapMut, text: &str, font: Font, x: i32, y: i32, color: Rgba) {
        if text.is_empty() || color.a == 0 {
            return;
        }
        if let Some(mask) = self.font.mask_weight(text, font.px * self.scale, font.weight) {
            blend_mask(pm, x, y, &mask, color);
        }
    }

    fn draw_text_centered(&self, pm: &mut PixmapMut, text: &str, font: Font, r: IRect, color: Rgba) {
        let (w, h) = self.measure(text, font);
        self.draw_text(pm, text, font, r.x + (r.w - w) / 2, r.y + (r.h - h) / 2, color);
    }

    /// `text` in lines no wider than `width`.
    fn wrap(&self, text: &str, font: Font, width: i32) -> Vec<String> {
        let mut lines = Vec::new();
        let mut line = String::new();
        for word in text.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if !line.is_empty() && self.measure(&candidate, font).0 > width {
                lines.push(std::mem::replace(&mut line, word.to_string()));
            } else {
                line = candidate;
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    }

    /// `text` cut to fit `width`, with an ellipsis where it was cut.
    fn elide(&self, text: &str, font: Font, width: i32) -> String {
        if self.measure(text, font).0 <= width {
            return text.to_string();
        }
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let candidate: String = chars.iter().collect::<String>() + "\u{2026}";
            if self.measure(&candidate, font).0 <= width {
                return candidate;
            }
        }
        String::new()
    }
}

// --- Layout -----------------------------------------------------------------------------------

struct Layout<'a> {
    s: &'a State,
    scene: Scene,
    y: i32,
    x: i32,
    w: i32,
    card_top: i32,
    /// Other descriptions the next row may show: it makes room for the longest.
    reserve: Vec<String>,
    /// Key caps for the next row.
    caps: Vec<String>,
}

impl Layout<'_> {
    fn px(&self, v: f32) -> i32 {
        self.s.px(v)
    }

    fn section(&mut self, title: &'static str, tab: Option<usize>) {
        if let Some(tab) = tab {
            self.scene.tab_tops[tab] = self.y;
        }
        self.scene.headings.push(Heading { text: title, x: self.x + self.px(4.0), y: self.y });
        self.y += self.px(32.0);
        self.card_top = self.y;
        self.y += self.px(CARD_PAD);
    }

    fn end_section(&mut self) {
        self.y += self.px(CARD_PAD) - self.px(1.0);
        self.scene.cards.push(IRect::new(self.x, self.card_top, self.w, self.y - self.card_top));
        self.y += self.px(SECTION_GAP);
    }

    /// A note under the card just ended.
    fn note(&mut self, text: &str, tone: Tone) {
        self.y -= self.px(SECTION_GAP) - self.px(10.0);
        let x = self.x + self.px(6.0);
        let lines = self.s.wrap(text, SMALL, self.w - self.px(12.0));
        let line_h = self.s.line_h(SMALL);
        self.scene.notes.push(Note { x, y: self.y, lines: lines.clone(), tone });
        self.y += line_h * lines.len() as i32 + self.px(SECTION_GAP);
    }

    fn row_x(&self) -> i32 {
        self.x + self.px(CARD_PAD)
    }

    fn row_w(&self) -> i32 {
        self.w - 2 * self.px(CARD_PAD)
    }

    /// A label and its description on the left, and a control on the right.
    fn row(&mut self, label: &str, desc: &str, tone: Tone, control: Option<(Ctl, Kind, i32, i32)>, toggles: bool) {
        let (x, w) = (self.row_x(), self.row_w());
        let pad = self.px(ROW_PAD);
        let control_w = control.as_ref().map(|c| c.2).unwrap_or(0);
        let caps = std::mem::take(&mut self.caps);
        let caps_w = if caps.is_empty() { 0 } else { caps_width(self.s, &caps) + self.px(20.0) };
        let text_w = w - 2 * pad - control_w - caps_w - if control.is_some() { self.px(28.0) } else { 0 };
        let desc_lines = if desc.is_empty() { Vec::new() } else { self.s.wrap(desc, SMALL, text_w) };
        let reserved = std::mem::take(&mut self.reserve).iter().map(|d| self.s.wrap(d, SMALL, text_w).len()).max().unwrap_or(0);
        let lines = desc_lines.len().max(reserved) as i32;
        let mut h = if lines == 0 { self.px(46.0) } else { self.px(31.0) + self.s.line_h(SMALL) * lines + self.px(12.0) };
        if let Some((_, _, _, ch)) = &control {
            h = h.max(ch + self.px(24.0));
        }
        let rect = IRect::new(x, self.y, w, h);
        let ctl = control.as_ref().map(|c| c.0);
        let caps_on = !matches!(control, Some((_, Kind::Toggle(false), _, _)));
        if let Some((ctl, kind, cw, ch)) = control {
            let r = IRect::new(x + w - pad - cw, self.y + (h - ch) / 2, cw, ch);
            self.scene.controls.push(Control { ctl, rect: r, kind });
        }
        let caps_right = x + w - pad - control_w - self.px(20.0);
        self.scene.rows.push(Row { rect, label: label.to_string(), label_x: x + pad, desc: desc_lines, tone, ctl, toggles, icon: None, caps, caps_right, caps_on });
        self.y += h + self.px(1.0);
    }

    fn toggle(&mut self, label: &str, desc: &str, tone: Tone, ctl: Ctl, on: bool) {
        let (w, h) = (self.px(38.0), self.px(22.0));
        self.row(label, desc, tone, Some((ctl, Kind::Toggle(on), w, h)), true);
    }

    fn segments(&mut self, label: &str, desc: &str, ctl: Ctl, segs: Vec<Seg>, selected: usize, seg_w: f32) {
        let seg_w = self.px(seg_w);
        let w = seg_w * segs.len() as i32 + 2 * self.px(SEG_INSET);
        let h = self.px(26.0) + 2 * self.px(SEG_INSET);
        self.row(label, desc, Tone::Subtle, Some((ctl, Kind::Segments { segs, selected, seg_w }, w, h)), false);
    }

    /// A label and description with a line of controls under them, for things that need room.
    fn stacked(&mut self, label: &str, desc: &str, tone: Tone, ctl: Ctl, controls: Vec<(Ctl, Kind, i32)>, extra_h: i32) -> IRect {
        let (x, w) = (self.row_x(), self.row_w());
        let pad = self.px(ROW_PAD);
        let desc_lines = self.s.wrap(desc, SMALL, w - 2 * pad);
        let top = self.y + self.px(31.0) + self.s.line_h(SMALL) * desc_lines.len() as i32 + self.px(10.0);
        let ch = self.px(CONTROL_H);
        let gap = self.px(8.0);
        // Fixed widths first; the one given 0 takes what's left.
        let fixed: i32 = controls.iter().map(|c| c.2).sum::<i32>() + gap * (controls.len() as i32 - 1);
        let mut cx = x + pad;
        for (c, kind, cw) in controls {
            let cw = if cw == 0 { w - 2 * pad - fixed } else { cw };
            self.scene.controls.push(Control { ctl: c, rect: IRect::new(cx, top, cw, ch), kind });
            cx += cw + gap;
        }
        let h = top + ch + extra_h + self.px(14.0) - self.y;
        let rect = IRect::new(x, self.y, w, h);
        self.scene.rows.push(Row { rect, label: label.to_string(), label_x: x + pad, desc: desc_lines, tone, ctl: Some(ctl), toggles: false, icon: None, caps: Vec::new(), caps_right: 0, caps_on: true });
        self.y += h + self.px(1.0);
        IRect::new(x + pad, top + ch, w - 2 * pad, extra_h)
    }
}

fn layout(s: &State) -> Scene {
    let (width, _) = s.size();
    let mut l = Layout { s, scene: Scene::default(), y: s.px(14.0), x: s.px(MARGIN), w: width - 2 * s.px(MARGIN), card_top: 0, reserve: Vec::new(), caps: Vec::new() };
    let set = &s.settings;

    // General
    l.section("General", Some(0));
    l.toggle(&format!("Start {} when I sign in", crate::util::APP_NAME), "It waits quietly in the tray.", Tone::Subtle, Ctl::Autostart, s.autostart);
    l.end_section();

    // Shortcuts: Windows' own capture keys first, then Prism's.
    l.section("Shortcuts", Some(1));
    let (snip, snip_tone) = match (&s.print_screen_problem, s.snipping_failed, set.replace_snipping) {
        (Some(problem), _, true) => (problem.clone(), Tone::Warning),
        (_, true, true) => (SNIPPING_FAILED.to_string(), Tone::Warning),
        (_, _, true) => (SNIPPING_ON.to_string(), Tone::Subtle),
        _ => (SNIPPING_OFF.to_string(), Tone::Subtle),
    };
    l.caps = SNIPPING_KEYS.iter().map(|k| k.to_string()).collect();
    l.toggle("Replace Snipping Tool", &snip, snip_tone, Ctl::Snipping, set.replace_snipping);
    for (slot, (label, about)) in SHORTCUTS.iter().enumerate() {
        let (desc, tone) = if s.recording == Some(slot) {
            (RECORDING_NOTE.to_string(), Tone::Accent)
        } else if let Some(problem) = &s.slot_notes[slot] {
            (problem.clone(), Tone::Warning)
        } else {
            (about.to_string(), Tone::Subtle)
        };
        let (w, h) = (s.px(250.0), s.px(CONTROL_H));
        l.row(label, &desc, tone, Some((Ctl::Recorder(slot), Kind::Recorder(slot), w, h)), false);
    }
    l.end_section();
    l.note(SHORTCUTS_NOTE, Tone::Muted);

    // Tool letters, in a grid.
    l.section("Tool letters", None);
    let reset_w = s.measure("Reset to defaults", CAP).0 + s.px(28.0);
    let heading_y = l.scene.headings.last().map(|h| h.y).unwrap_or(0);
    l.scene.controls.push(Control {
        ctl: Ctl::ResetToolKeys,
        rect: IRect::new(l.x + l.w - reset_w, heading_y - s.px(2.0), reset_w, s.px(28.0)),
        kind: Kind::Button { label: "Reset to defaults", ghost: false },
    });
    let (gx, gw) = (l.row_x(), l.row_w());
    let gap = s.px(1.0);
    let cell_w = (gw - gap * (TOOL_COLUMNS - 1)) / TOOL_COLUMNS;
    let cell_h = s.px(46.0);
    let key = s.px(30.0);
    for (i, tool) in Tool::ALL.iter().enumerate() {
        let (col, row) = (i as i32 % TOOL_COLUMNS, i as i32 / TOOL_COLUMNS);
        let rect = IRect::new(gx + col * (cell_w + gap), l.y + row * (cell_h + gap), cell_w, cell_h);
        let pad = s.px(ROW_PAD);
        l.scene.controls.push(Control {
            ctl: Ctl::ToolKey(i),
            rect: IRect::new(rect.right() - pad - key, rect.y + (cell_h - key) / 2, key, key),
            kind: Kind::ToolKey(i),
        });
        l.scene.rows.push(Row {
            rect,
            label: tool.label().to_string(),
            label_x: rect.x + pad + s.px(30.0),
            desc: Vec::new(),
            tone: Tone::Subtle,
            ctl: Some(Ctl::ToolKey(i)),
            toggles: false,
            icon: Some(*tool),
            caps: Vec::new(),
            caps_right: 0,
            caps_on: true,
        });
    }
    let rows = (Tool::ALL.len() as i32 + TOOL_COLUMNS - 1) / TOOL_COLUMNS;
    l.y += rows * (cell_h + gap);
    l.end_section();
    match (&s.tool_note, s.tool_listening) {
        (Some(problem), _) => l.note(problem, Tone::Warning),
        (None, Some(_)) => l.note(TOOL_LISTENING, Tone::Accent),
        (None, None) => l.note(TOOLS_NOTE, Tone::Muted),
    }

    // Capturing
    l.section("While you select", Some(2));
    l.toggle("Show the magnifier", "A close-up of the pixels under the pointer, with their colour. Press C to copy the colour.", Tone::Subtle, Ctl::Magnifier, set.magnifier);
    l.toggle("Include the mouse pointer", "In region and screen captures.", Tone::Subtle, Ctl::Cursor, set.capture_cursor);
    l.end_section();

    l.section("After you select", None);
    let quick = QUICK.iter().position(|(_, q)| *q == set.quick_finish).unwrap_or(0);
    l.reserve = (0..QUICK.len()).map(quick_desc).collect();
    l.segments("Finish straight away", &quick_desc(quick), Ctl::QuickFinish, QUICK.iter().map(|(n, _)| Seg::Text(n)).collect(), quick, 58.0);
    let chosen = auto_tool_choice(set.auto_tool);
    let shown = match s.hover {
        Some((Ctl::AutoTool, part)) => part,
        _ => chosen,
    };
    l.reserve = (0..Tool::ALL.len()).map(tool_desc).collect();
    l.segments("Pick up a tool", &tool_desc(shown), Ctl::AutoTool, Tool::ALL.iter().map(|t| Seg::Icon(*t)).collect(), chosen, 31.0);
    l.end_section();

    l.section("When you're done", None);
    let finish = FINISH_KEYS.iter().position(|(_, k)| *k == set.finish_key).unwrap_or(0);
    l.reserve = (0..FINISH_KEYS.len()).map(|i| finish_desc(i).to_string()).collect();
    l.segments("Finish with", finish_desc(finish), Ctl::FinishKey, FINISH_KEYS.iter().map(|(n, _)| Seg::Text(n)).collect(), finish, 64.0);
    let enter = ENTER.iter().position(|(_, a)| *a == set.enter_action).unwrap_or(0);
    l.reserve = (0..ENTER.len()).map(|i| enter_desc(i).to_string()).collect();
    l.segments("Finishing does", enter_desc(enter), Ctl::EnterAction, ENTER.iter().map(|(n, _)| Seg::Text(n)).collect(), enter, 64.0);
    l.toggle("Also save what you copy", "Copied captures go to your folder too, so nothing is lost.", Tone::Subtle, Ctl::SaveCopies, set.save_copies);
    l.end_section();

    l.section("Drawing", None);
    let colors = color_choices(&s.settings);
    let color = s.settings.start_color.and_then(|c| colors.iter().position(|x| *x == c)).map_or(0, |i| i + 1);
    let shown = match s.hover {
        Some((Ctl::StartColor, part)) => part,
        _ => color,
    };
    let color_desc = |i: usize| match i.checked_sub(1).and_then(|i| colors.get(i)) {
        None => "Each capture starts with the colour you drew with last.".to_string(),
        Some(c) => format!("Each capture starts in {}. Keys 1 to 8 still change it as you draw.", color_name(*c)),
    };
    l.reserve = (0..=colors.len()).map(color_desc).collect();
    let segs = std::iter::once(Seg::Text("Last")).chain(colors.iter().map(|c| Seg::Swatch(*c))).collect();
    l.segments("Start with this colour", &color_desc(shown), Ctl::StartColor, segs, color, 34.0);
    let widths = width_choices(&s.settings);
    let width = s.settings.start_width.and_then(|w| widths.iter().position(|x| (*x - w).abs() < 0.01)).map_or(0, |i| i + 1);
    let shown = match s.hover {
        Some((Ctl::StartWidth, part)) => part,
        _ => width,
    };
    let width_desc = |i: usize| match i.checked_sub(1).and_then(|i| widths.get(i)) {
        None => "Each capture starts with the size you drew with last.".to_string(),
        Some(w) => format!("Each capture starts at size {}. The scroll wheel still changes it as you draw.", *w as i32),
    };
    l.reserve = (0..=widths.len()).map(width_desc).collect();
    let segs = std::iter::once(Seg::Text("Last")).chain(widths.iter().map(|w| Seg::Dot(*w))).collect();
    l.segments("Start with this size", &width_desc(shown), Ctl::StartWidth, segs, width, 34.0);
    l.end_section();

    l.section("Popups", None);
    l.toggle("Show a popup after copying or saving", "A small card with a thumbnail. Click it to show the file.", Tone::Subtle, Ctl::Notify, set.notify);
    let spot = POPUP_POSITIONS.iter().position(|(_, p)| *p == set.popup_position).unwrap_or(5);
    let shown = match s.hover {
        Some((Ctl::PopupPosition, part)) => part,
        _ => spot,
    };
    let (pw, ph) = (s.px(84.0), s.px(60.0));
    l.reserve = (0..POPUP_POSITIONS.len()).map(where_desc).collect();
    l.row("Where popups appear", &where_desc(shown), Tone::Subtle, Some((Ctl::PopupPosition, Kind::Picker(spot), pw, ph)), false);
    l.end_section();

    // Saving
    l.section("Saving", Some(3));
    let browse_w = s.measure("Browse\u{2026}", STRONG).0 + s.px(36.0);
    let open_w = s.measure("Open folder", STRONG).0 + s.px(36.0);
    l.stacked(
        "Folder",
        "Left empty, captures go to your Screenshots folder.",
        Tone::Subtle,
        Ctl::Folder,
        vec![
            (Ctl::Folder, Kind::Field(Field::Folder), 0),
            (Ctl::Browse, Kind::Button { label: "Browse\u{2026}", ghost: false }, browse_w),
            (Ctl::OpenFolder, Kind::Button { label: "Open folder", ghost: false }, open_w),
        ],
        0,
    );
    let chips_h = s.px(40.0);
    let below = l.stacked("File name", "The date, time and size fill in when you save.", Tone::Subtle, Ctl::FileName, vec![(Ctl::FileName, Kind::Field(Field::FileName), 0)], chips_h);
    let chip_h = s.px(26.0);
    let mut cx = below.x;
    for (i, (name, _)) in CHIPS.iter().enumerate() {
        let w = s.measure(&format!("+ {name}"), CAP).0 + s.px(22.0);
        l.scene.controls.push(Control {
            ctl: Ctl::Chip(i),
            rect: IRect::new(cx, below.y + s.px(10.0), w, chip_h),
            kind: Kind::Button { label: CHIPS[i].0, ghost: true },
        });
        cx += w + s.px(6.0);
    }
    l.end_section();

    l.y += s.px(4.0);
    let footer = format!("{} {} \u{00B7} Changes save as you make them", crate::util::APP_NAME, env!("CARGO_PKG_VERSION"));
    l.scene.notes.push(Note { x: -1, y: l.y, lines: vec![footer], tone: Tone::Muted });
    l.y += s.line_h(SMALL) + s.px(28.0);
    l.scene.height = l.y;
    l.scene
}

// Descriptions that change with the choice. Rows make room for the longest, so they keep
// their height as the choice, or the one pointed at, changes.

fn quick_desc(i: usize) -> String {
    match QUICK[i.min(QUICK.len() - 1)] {
        (_, QuickFinish::Off) => "The toolbar always opens, ready to mark up.".to_string(),
        (_, QuickFinish::Always) => "Every capture finishes the moment it's chosen, with no marking up.".to_string(),
        (key, _) => format!("Hold {key} as you let go of the mouse to finish at once, without the toolbar."),
    }
}

fn tool_desc(i: usize) -> String {
    match auto_tool_of(i) {
        None => "None: Select and move is in hand, and drawing tools wait on the toolbar.".to_string(),
        Some(tool) => format!("{}, as soon as a region is chosen. Dragging outside the region chooses a new one.", tool.label()),
    }
}

fn finish_desc(i: usize) -> &'static str {
    match FINISH_KEYS[i.min(FINISH_KEYS.len() - 1)].1 {
        FinishKey::Enter => "Esc cancels. Double-clicking the region finishes too.",
        FinishKey::Escape => "Enter does nothing, and the toolbar's close button cancels.",
        FinishKey::Both => "Either key finishes, and the toolbar's close button cancels.",
    }
}

fn enter_desc(i: usize) -> &'static str {
    match ENTER[i.min(ENTER.len() - 1)].1 {
        EnterAction::Copy => "Copy to the clipboard. Ctrl+C always copies, and Ctrl+S always saves.",
        EnterAction::Save => "Save to your folder. Ctrl+C always copies, and Ctrl+S always saves.",
        EnterAction::CopyAndSave => "Copy and save. Ctrl+C always copies, and Ctrl+S always saves.",
    }
}

fn where_desc(i: usize) -> String {
    format!("{}, on the screen you captured. Problems with Prism show here even with popups off.", POPUP_POSITIONS[i.min(POPUP_POSITIONS.len() - 1)].0)
}

/// What the next file will be called.
fn example_name(s: &State) -> String {
    let pattern = if s.settings.file_name.trim().is_empty() { Settings::default().file_name } else { s.settings.file_name.clone() };
    format!("{}.png", naming::format_name(&pattern, crate::util::now_stamp(), 1920, 1080))
}

// --- Drawing ----------------------------------------------------------------------------------

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(paint_color(c));
    p.anti_alias = true;
    p
}

fn fill_rr(pm: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba) {
    if c.a == 0 || w <= 0.0 || h <= 0.0 {
        return;
    }
    if let Some(path) = rounded_rect(x, y, w, h, r) {
        pm.fill_path(&path, &paint(c), FillRule::Winding, Transform::identity(), None);
    }
}

fn stroke_rr(pm: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32, width: f32, c: Rgba) {
    if c.a == 0 {
        return;
    }
    let half = width / 2.0;
    if let Some(path) = rounded_rect(x + half, y + half, w - width, h - width, (r - half).max(0.0)) {
        pm.stroke_path(&path, &paint(c), &Stroke { width, ..Stroke::default() }, Transform::identity(), None);
    }
}

fn rect_f(r: IRect) -> (f32, f32, f32, f32) {
    (r.x as f32, r.y as f32, r.w as f32, r.h as f32)
}

/// Glass: an even, faint fill and a thin rim.
fn glass(s: &State, pm: &mut PixmapMut, r: IRect, radius: f32, strength: f32) {
    let p = &s.palette;
    let (x, y, w, h) = rect_f(r);
    fill_rr(pm, x, y, w, h, radius, p.ink(0.04 * strength));
    stroke_rr(pm, x, y, w, h, radius, s.scale.max(1.0).round(), p.line(0.1 * strength));
}

fn line(pm: &mut PixmapMut, a: (f32, f32), b: (f32, f32), width: f32, c: Rgba) {
    let mut pb = PathBuilder::new();
    pb.move_to(a.0, a.1);
    pb.line_to(b.0, b.1);
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &paint(c), &Stroke { width, line_cap: LineCap::Round, ..Stroke::default() }, Transform::identity(), None);
    }
}

fn render(s: &mut State) {
    if s.scene_stale {
        s.scene = layout(s);
        s.scene_stale = false;
        s.scroll_to = s.scroll_to.clamp(0.0, s.max_scroll());
        s.scroll = s.scroll.clamp(0.0, s.max_scroll());
    }
    let Some(mut canvas) = s.canvas.take() else { return };
    {
        let mut pm = canvas.pixmap();
        draw(s, &mut pm);
    }
    s.canvas = Some(canvas);
}

fn draw(s: &mut State, pm: &mut PixmapMut) {
    let p = s.palette;
    pm.fill(paint_color(p.bg));
    let (width, height) = s.size();
    let header = s.header_h();

    // The page, under the header.
    let scene = std::mem::take(&mut s.scene);
    let visible = |r: IRect, s: &State| {
        let r = s.on_screen(r);
        r.bottom() > header && r.y < height
    };
    for card in &scene.cards {
        if visible(*card, s) {
            glass(s, pm, s.on_screen(*card), s.px(13.0) as f32, 0.8);
        }
    }
    for heading in &scene.headings {
        let y = heading.y + header - s.scroll.round() as i32;
        if y + s.px(30.0) > header && y < height {
            s.draw_text(pm, heading.text, HEADING, heading.x, y, p.text);
        }
    }
    for row in &scene.rows {
        draw_row(s, pm, row, header, height);
    }
    for control in &scene.controls {
        if visible(control.rect, s) {
            draw_control(s, pm, control);
        }
    }
    for note in &scene.notes {
        let line_h = s.line_h(SMALL);
        for (i, text) in note.lines.iter().enumerate() {
            let y = note.y + header - s.scroll.round() as i32 + i as i32 * line_h;
            if y + line_h > header && y < height {
                let x = if note.x < 0 { (width - s.measure(text, SMALL).0) / 2 } else { note.x };
                s.draw_text(pm, text, SMALL, x, y, p.tone(note.tone));
            }
        }
    }
    if let Some(focus) = s.focus.filter(|_| s.focus_ring) {
        if let Some(c) = scene.controls.iter().find(|c| c.ctl == focus) {
            if visible(c.rect, s) {
                let r = s.on_screen(c.rect).inflate(s.px(3.0));
                let radius = control_radius(s, c) + s.px(3.0) as f32;
                let (x, y, w, h) = rect_f(r);
                stroke_rr(pm, x, y, w, h, radius, (2.0 * s.scale).round().max(2.0), alpha(p.accent, 0.9));
            }
        }
    }
    s.scene = scene;

    draw_scrollbar(s, pm);
    draw_header(s, pm);
}

fn control_radius(s: &State, c: &Control) -> f32 {
    match c.kind {
        Kind::Toggle(_) | Kind::Segments { .. } | Kind::Button { .. } => c.rect.h as f32 / 2.0,
        Kind::ToolKey(_) => s.px(8.0) as f32,
        Kind::Picker(_) => s.px(8.0) as f32,
        _ => s.px(9.0) as f32,
    }
}

fn draw_row(s: &mut State, pm: &mut PixmapMut, row: &Row, header: i32, height: i32) {
    let p = s.palette;
    let r = s.on_screen(row.rect);
    if r.bottom() <= header || r.y >= height {
        return;
    }
    if let Some(ctl) = row.ctl {
        let lit = s.hover_row == Some(ctl) && s.thumb_grab.is_none();
        let shown = s.anims.follow(Key::Hover(ctl), if lit { 1.0 } else { 0.0 }, HOVER);
        if shown > 0.004 {
            let inner = IRect::new(r.x, r.y + s.px(1.0), r.w, r.h - s.px(2.0));
            let radius = s.px(9.0) as f32;
            let (x, y, w, h) = rect_f(inner);
            fill_rr(pm, x, y, w, h, radius, p.ink(0.04 * 0.7 * shown));
            stroke_rr(pm, x, y, w, h, radius, s.scale.max(1.0).round(), p.line(0.1 * 0.7 * shown));
        }
    }
    let label_y = if row.desc.is_empty() { r.y + (r.h - s.line_h(LABEL)) / 2 } else { r.y + s.px(11.0) };
    if let Some(tool) = row.icon {
        let box_ = IRect::new(row.label_x - s.px(30.0), r.y, s.px(22.0), r.h);
        ui::icon(pm, &s.font, Btn::Tool(tool), box_, p.secondary, s.scale, IconExtra { color: p.secondary, width: 4.0 });
    }
    s.draw_text(pm, &row.label, LABEL, row.label_x, label_y, p.text);
    if !row.caps.is_empty() {
        draw_cap_groups(s, pm, &row.caps, row.caps_right, r.y + r.h / 2, if row.caps_on { 1.0 } else { 0.4 });
    }
    let line_h = s.line_h(SMALL);
    for (i, text) in row.desc.iter().enumerate() {
        s.draw_text(pm, text, SMALL, row.label_x, r.y + s.px(31.0) + i as i32 * line_h, p.tone(row.tone));
    }
}

fn draw_control(s: &mut State, pm: &mut PixmapMut, c: &Control) {
    let p = s.palette;
    let r = s.on_screen(c.rect);
    let (x, y, w, h) = rect_f(r);
    let hovered = matches!(s.hover, Some((ctl, _)) if ctl == c.ctl);
    let pressed = matches!(s.pressed, Some((ctl, _)) if ctl == c.ctl) && hovered;
    match &c.kind {
        Kind::Toggle(on) => {
            let k = s.anims.follow(Key::Slide(c.ctl), if *on { 1.0 } else { 0.0 }, Motion::Spring);
            let press = s.anims.follow(Key::Pressed(c.ctl), if pressed { 1.0 } else { 0.0 }, QUICK_FADE);
            let track = mix(p.ink(0.13), p.accent, k);
            fill_rr(pm, x, y, w, h, h / 2.0, track);
            if p.high_contrast {
                stroke_rr(pm, x, y, w, h, h / 2.0, 1.0, p.text);
            }
            let inset = 3.0 * s.scale;
            let knob_h = h - 2.0 * inset;
            let knob_w = knob_h + 4.0 * s.scale * press;
            let kx = x + inset + k * (w - 2.0 * inset - knob_w);
            fill_rr(pm, kx, y + inset, knob_w, knob_h, knob_h / 2.0, mix(p.ink(0.8), p.on_accent, k));
        }
        Kind::Segments { segs, selected, seg_w } => {
            fill_rr(pm, x, y, w, h, h / 2.0, p.ink(0.05));
            if p.high_contrast {
                stroke_rr(pm, x, y, w, h, h / 2.0, 1.0, p.text);
            }
            let inset = s.px(SEG_INSET) as f32;
            let at = s.anims.follow(Key::Slide(c.ctl), *selected as f32, Motion::Spring);
            let (pill_x, pill_h) = (x + inset + at * *seg_w as f32, h - 2.0 * inset);
            fill_rr(pm, pill_x, y + inset, *seg_w as f32, pill_h, pill_h / 2.0, if p.high_contrast { p.accent } else { p.ink(0.11) });
            stroke_rr(pm, pill_x, y + inset, *seg_w as f32, pill_h, pill_h / 2.0, s.scale.max(1.0).round(), p.line(0.06));
            for (i, seg) in segs.iter().enumerate() {
                let cell = IRect::new(r.x + s.px(SEG_INSET) + i as i32 * seg_w, r.y + s.px(SEG_INSET), *seg_w, r.h - 2 * s.px(SEG_INSET));
                let lit = s.anims.follow(Key::HoverPart(c.ctl, i), if s.hover == Some((c.ctl, i)) { 1.0 } else { 0.0 }, QUICK_FADE);
                let color = if i == *selected {
                    if p.high_contrast { p.on_accent } else { p.text }
                } else {
                    mix(p.subtle, p.text, lit * 0.6)
                };
                match seg {
                    Seg::Text(label) => s.draw_text_centered(pm, label, STRONG, cell, color),
                    Seg::Icon(tool) => {
                        let icon_scale = s.scale * 0.92;
                        ui::icon(pm, &s.font, Btn::Tool(*tool), cell, color, icon_scale, IconExtra { color, width: 4.0 });
                    }
                    Seg::Swatch(swatch) => {
                        let d = 14.0 * s.scale;
                        let (cx, cy) = (cell.x as f32 + cell.w as f32 / 2.0, cell.y as f32 + cell.h as f32 / 2.0);
                        fill_rr(pm, cx - d / 2.0, cy - d / 2.0, d, d, d / 2.0, *swatch);
                        // A rim, so black shows on dark and white on light.
                        stroke_rr(pm, cx - d / 2.0, cy - d / 2.0, d, d, d / 2.0, s.scale.max(1.0), p.line(0.3));
                    }
                    Seg::Dot(width) => {
                        let d = (2.5 + width * 0.85) * s.scale;
                        let (cx, cy) = (cell.x as f32 + cell.w as f32 / 2.0, cell.y as f32 + cell.h as f32 / 2.0);
                        fill_rr(pm, cx - d / 2.0, cy - d / 2.0, d, d, d / 2.0, color);
                    }
                }
            }
        }
        Kind::Button { label, ghost } => {
            let lit = s.anims.follow(Key::Hover(c.ctl), if hovered { 1.0 } else { 0.0 }, QUICK_FADE);
            let base = if *ghost { 0.0 } else { 0.07 };
            let fill = if pressed { 0.15 } else { base + (0.11 - base) * lit };
            fill_rr(pm, x, y, w, h, h / 2.0, p.ink(fill));
            if !*ghost || lit > 0.0 {
                stroke_rr(pm, x, y, w, h, h / 2.0, s.scale.max(1.0).round(), p.line(0.06 * if *ghost { lit } else { 1.0 }));
            }
            let text = if matches!(c.ctl, Ctl::Chip(_)) { format!("+ {label}") } else { label.to_string() };
            let font = if matches!(c.ctl, Ctl::Chip(_) | Ctl::ResetToolKeys) { CAP } else { STRONG };
            // Nothing to reset: the button says so by being quiet.
            let idle = c.ctl == Ctl::ResetToolKeys && s.settings.tool_keys == ToolKeys::default();
            let color = if *ghost { mix(p.subtle, p.text, lit) } else if idle { mix(p.muted, p.subtle, lit) } else { p.text };
            s.draw_text_centered(pm, &text, font, r, color);
        }
        Kind::Recorder(slot) => {
            let listening = s.recording == Some(*slot);
            let lit = s.anims.follow(Key::Hover(c.ctl), if hovered || listening { 1.0 } else { 0.0 }, QUICK_FADE);
            let radius = s.px(9.0) as f32;
            fill_rr(pm, x, y, w, h, radius, p.ink(0.05 + 0.04 * lit));
            let border = if listening { alpha(p.accent, 0.85) } else { p.line(0.06) };
            stroke_rr(pm, x, y, w, h, radius, s.scale.max(1.0).round(), border);
            let pad = s.px(10.0);
            if listening {
                if s.held.is_empty() {
                    s.draw_text(pm, "Press keys\u{2026}", BODY, r.x + pad + s.px(2.0), r.y + (r.h - s.line_h(BODY)) / 2, p.accent);
                } else {
                    let held = s.held.clone();
                    draw_caps(s, pm, &held, r.x + pad - s.px(2.0), r.y + r.h / 2, 1.0);
                }
                return;
            }
            let keys = shortcut(&s.settings.hotkeys, *slot).to_string();
            if keys.trim().is_empty() {
                s.draw_text(pm, "Off", BODY, r.x + pad + s.px(2.0), r.y + (r.h - s.line_h(BODY)) / 2, p.muted);
            } else {
                draw_caps(s, pm, &keys, r.x + pad - s.px(2.0), r.y + r.h / 2, 1.0);
            }
            // "Default", or a way back to it.
            let (tag, color) = if is_default(s, *slot) {
                ("Default", p.muted)
            } else {
                let lit = s.anims.follow(Key::HoverPart(c.ctl, 1), if s.hover == Some((c.ctl, 1)) { 1.0 } else { 0.0 }, QUICK_FADE);
                ("Reset", mix(p.subtle, p.accent, lit))
            };
            let (tw, th) = s.measure(tag, SMALL);
            s.draw_text(pm, tag, SMALL, r.right() - pad - tw, r.y + (r.h - th) / 2, color);
        }
        Kind::ToolKey(i) => {
            let listening = s.tool_listening == Some(*i);
            let lit = s.anims.follow(Key::Hover(c.ctl), if hovered || listening { 1.0 } else { 0.0 }, QUICK_FADE);
            let radius = s.px(8.0) as f32;
            // A key: a darker edge under its face.
            fill_rr(pm, x, y + s.scale, w, h, radius, p.ink(0.10 + 0.04 * lit));
            fill_rr(pm, x, y, w, h - s.scale * 1.5, radius, over(over(p.bg, p.ink(0.04)), p.ink(0.08 + 0.05 * lit)));
            let border = if listening { alpha(p.accent, 0.9) } else { p.line(0.08) };
            stroke_rr(pm, x, y, w, h, radius, if listening { (1.5 * s.scale).max(1.5) } else { s.scale.max(1.0).round() }, border);
            if listening {
                s.draw_text_centered(pm, "?", STRONG, r, p.accent);
            } else {
                let letter = s.settings.tool_keys.key(Tool::ALL[*i]).to_string();
                s.draw_text_centered(pm, &letter, Font { px: 14.0, weight: 600 }, r, p.text);
            }
        }
        Kind::Field(field) => {
            let editing = s.editing == Some(*field);
            let lit = s.anims.follow(Key::Hover(c.ctl), if hovered && !editing { 1.0 } else { 0.0 }, QUICK_FADE);
            let radius = s.px(9.0) as f32;
            fill_rr(pm, x, y, w, h, radius, p.ink(if editing { 0.09 } else { 0.05 + 0.03 * lit }));
            stroke_rr(pm, x, y, w, h, radius, s.scale.max(1.0).round(), if editing { p.line(0.24) } else { p.line(0.06) });
            if editing && !s.preview {
                return;
            }
            let pad = s.px(12.0);
            let (value, hint) = match field {
                Field::Folder => (s.settings.save_dir.clone(), crate::util::default_save_dir().display().to_string()),
                Field::FileName => (s.settings.file_name.clone(), Settings::default().file_name),
            };
            let (text, color) = if value.is_empty() { (hint, p.muted) } else { (value, p.text) };
            let text = s.elide(&text, BODY, r.w - 2 * pad);
            s.draw_text(pm, &text, BODY, r.x + pad, r.y + (r.h - s.line_h(BODY)) / 2, color);
            if *field == Field::FileName {
                // What the next file will be called, at the right of the line below.
                let example = example_name(s);
                let shown = s.elide(&example, SMALL, r.w / 2);
                let (tw, _) = s.measure(&shown, SMALL);
                let line_y = r.bottom() + s.px(10.0) + (s.px(26.0) - s.line_h(SMALL)) / 2;
                s.draw_text(pm, &shown, SMALL, r.right() - tw, line_y, p.muted);
            }
        }
        Kind::Picker(spot) => draw_picker(s, pm, c, r, *spot),
    }
}

/// The spots of the popup picker, in `POPUP_POSITIONS` order, inside the screen `r`.
fn picker_spots(s: &State, r: IRect) -> [IRect; 6] {
    let screen = picker_screen(s, r);
    let (cw, ch) = (screen.w / 3, screen.h / 2);
    std::array::from_fn(|i| IRect::new(screen.x + (i as i32 % 3) * cw, screen.y + (i as i32 / 3) * ch, cw, ch))
}

fn picker_screen(s: &State, r: IRect) -> IRect {
    IRect::new(r.x, r.y, r.w, r.h - s.px(10.0))
}

fn draw_picker(s: &mut State, pm: &mut PixmapMut, c: &Control, r: IRect, chosen: usize) {
    let p = s.palette;
    let screen = picker_screen(s, r);
    let (x, y, w, h) = rect_f(screen);
    let radius = s.px(7.0) as f32;
    fill_rr(pm, x, y, w, h, radius, p.ink(0.05));
    stroke_rr(pm, x, y, w, h, radius, s.scale.max(1.0).round(), p.line(0.16));
    // The stand.
    let stand_w = s.px(22.0) as f32;
    fill_rr(pm, x + (w - stand_w) / 2.0, y + h + s.px(4.0) as f32, stand_w, s.px(3.0) as f32, s.px(1.5) as f32, p.ink(0.18));
    let pad = s.px(5.0) as f32;
    let (toast_w, toast_h) = (s.px(20.0) as f32, s.px(7.0) as f32);
    for (i, spot) in picker_spots(s, r).iter().enumerate() {
        let lit = s.anims.follow(Key::HoverPart(c.ctl, i), if s.hover == Some((c.ctl, i)) { 1.0 } else { 0.0 }, QUICK_FADE);
        let col = i % 3;
        let top = i < 3;
        let tx = match col {
            0 => x + pad,
            1 => x + (w - toast_w) / 2.0,
            _ => x + w - pad - toast_w,
        };
        let ty = if top { y + pad } else { y + h - pad - toast_h };
        if i == chosen {
            fill_rr(pm, tx, ty, toast_w, toast_h, toast_h / 2.0, p.accent);
        } else if lit > 0.01 {
            fill_rr(pm, tx, ty, toast_w, toast_h, toast_h / 2.0, p.ink(0.35 * lit));
        } else {
            let dot = s.px(4.0) as f32;
            let (cx, cy) = (tx + toast_w / 2.0, ty + toast_h / 2.0);
            fill_rr(pm, cx - dot / 2.0, cy - dot / 2.0, dot, dot, dot / 2.0, p.ink(0.22));
        }
        let _ = spot;
    }
}

const CAP_H: f32 = 22.0;
const CAP_PAD: f32 = 7.0;
const CAP_GAP: f32 = 4.0;

/// How wide one key cap is.
fn cap_width(s: &State, key: &str) -> i32 {
    (s.measure(hotkey::cap_label(key), CAP).0 + s.px(CAP_PAD) * 2).max(s.px(CAP_H))
}

/// How wide the caps for a shortcut such as "Ctrl+Alt+S" are.
fn keys_width(s: &State, keys: &str) -> i32 {
    let caps: Vec<i32> = keys.split('+').map(|k| cap_width(s, k.trim())).collect();
    caps.iter().sum::<i32>() + s.px(CAP_GAP) * (caps.len() as i32 - 1).max(0)
}

/// How wide shortcuts side by side are, with "or" between them.
fn caps_width(s: &State, groups: &[String]) -> i32 {
    let or = s.measure("or", SMALL).0 + 2 * s.px(8.0);
    groups.iter().map(|g| keys_width(s, g)).sum::<i32>() + or * (groups.len() as i32 - 1).max(0)
}

/// Key caps for a shortcut such as "Ctrl+Alt+S", from `x`, centred on `middle_y`, at an opacity.
fn draw_caps(s: &State, pm: &mut PixmapMut, keys: &str, x: i32, middle_y: i32, opacity: f32) {
    let p = s.palette;
    let height = s.px(CAP_H);
    let top = middle_y - height / 2;
    let mut at = x;
    for key in keys.split('+').map(str::trim) {
        let label = hotkey::cap_label(key);
        let width = cap_width(s, key);
        let (fx, fy, fw, fh) = (at as f32, top as f32, width as f32, height as f32);
        let radius = s.px(6.0) as f32;
        fill_rr(pm, fx, fy + s.scale, fw, fh, radius, alpha(p.ink(0.14), opacity));
        fill_rr(pm, fx, fy, fw, fh - s.scale, radius, alpha(over(over(p.bg, p.ink(0.07)), p.ink(0.1)), opacity));
        stroke_rr(pm, fx, fy, fw, fh, radius, s.scale.max(1.0).round(), alpha(p.line(0.07), opacity));
        s.draw_text_centered(pm, label, CAP, IRect::new(at, top, width, height - s.px(1.0)), alpha(p.text, opacity));
        at += width + s.px(CAP_GAP);
    }
}

/// Shortcuts side by side with "or" between them, ending at `right`.
fn draw_cap_groups(s: &State, pm: &mut PixmapMut, groups: &[String], right: i32, middle_y: i32, opacity: f32) {
    let mut at = right - caps_width(s, groups);
    let gap = s.px(8.0);
    for (i, group) in groups.iter().enumerate() {
        if i > 0 {
            let (tw, th) = s.measure("or", SMALL);
            s.draw_text(pm, "or", SMALL, at + gap, middle_y - th / 2, alpha(s.palette.muted, opacity));
            at += tw + 2 * gap;
        }
        draw_caps(s, pm, group, at, middle_y, opacity);
        at += keys_width(s, group);
    }
}

fn draw_scrollbar(s: &mut State, pm: &mut PixmapMut) {
    let Some((thumb_y, thumb_h)) = thumb(s) else { return };
    let shown = s.anims.follow(Key::Bar, if s.near_bar || s.thumb_grab.is_some() || s.anims.value(Key::Bar) > 0.0 && bar_awake(s) { 1.0 } else { 0.0 }, SLOW_FADE);
    if shown <= 0.004 {
        return;
    }
    let p = s.palette;
    let (width, _) = s.size();
    let wide_bar = s.near_bar || s.thumb_grab.is_some();
    let bw = s.px(if wide_bar { 6.0 } else { 4.0 }) as f32;
    let x = width as f32 - s.px(5.0) as f32 - bw;
    fill_rr(pm, x, thumb_y as f32, bw, thumb_h as f32, bw / 2.0, p.ink((if wide_bar { 0.34 } else { 0.24 }) * shown));
}

thread_local! {
    static BAR_AWAKE: Cell<bool> = const { Cell::new(false) };
}

fn bar_awake(_s: &State) -> bool {
    BAR_AWAKE.with(|b| b.get())
}

/// Where the scroll thumb is, top and height, or None when nothing scrolls.
fn thumb(s: &State) -> Option<(i32, i32)> {
    let max = s.max_scroll();
    if max <= 0.0 {
        return None;
    }
    let (_, height) = s.size();
    let inset = s.px(8.0);
    let track_top = s.header_h() + inset;
    let track = height - track_top - inset;
    let view = s.view_h() as f32;
    let h = ((view / s.scene.height as f32) * track as f32).max(s.px(36.0) as f32) as i32;
    let y = track_top + ((s.scroll / max) * (track - h) as f32).round() as i32;
    Some((y, h))
}

fn current_tab(s: &State) -> usize {
    if let Some(tab) = s.pinned_tab {
        return tab;
    }
    let max = s.max_scroll();
    if max > 0.0 && s.scroll_to >= max - 1.0 {
        return TABS.len() - 1;
    }
    let read = s.scroll_to.round() as i32 + s.px(60.0);
    (0..TABS.len()).rev().find(|t| s.scene.tab_tops[*t] <= read).unwrap_or(0)
}

fn tab_rects(s: &State) -> (IRect, Vec<IRect>) {
    let inset = s.px(SEG_INSET);
    let (tw, th) = (s.px(TAB_W), s.px(TAB_H));
    let outer = IRect::new(s.px(MARGIN), s.px(TITLE_H), tw * TABS.len() as i32 + 2 * inset, th + 2 * inset);
    let cells = (0..TABS.len()).map(|i| IRect::new(outer.x + inset + i as i32 * tw, outer.y + inset, tw, th)).collect();
    (outer, cells)
}

fn caption_rects(s: &State) -> [(Ctl, IRect); 2] {
    let (width, _) = s.size();
    let (w, h) = (s.px(CAPTION_W), s.px(CAPTION_H));
    [(Ctl::Minimize, IRect::new(width - 2 * w, 0, w, h)), (Ctl::Close, IRect::new(width - w, 0, w, h))]
}

fn draw_header(s: &mut State, pm: &mut PixmapMut) {
    let p = s.palette;
    let (width, _) = s.size();
    let header = s.header_h();
    ui::fill_rect(pm, IRect::new(0, 0, width, header), p.bg);
    let title_h = s.px(TITLE_H);
    s.draw_text(pm, "Settings", TITLE, s.px(MARGIN + 4.0), (title_h - s.line_h(TITLE)) / 2 + s.px(4.0), p.text);

    // Caption buttons.
    for (ctl, r) in caption_rects(s) {
        let lit = s.anims.follow(Key::Hover(ctl), if s.hover.map(|h| h.0) == Some(ctl) { 1.0 } else { 0.0 }, QUICK_FADE);
        let close = ctl == Ctl::Close;
        let pressed = s.pressed.map(|h| h.0) == Some(ctl) && lit > 0.5;
        if lit > 0.0 {
            let fill = if close { alpha(rgb(0xE8, 0x11, 0x23), lit * if pressed { 0.8 } else { 1.0 }) } else { p.ink(if pressed { 0.12 } else { 0.08 } * lit) };
            ui::fill_rect(pm, r, fill);
        }
        let ink = if close { mix(p.text, rgb(0xFF, 0xFF, 0xFF), lit) } else { p.text };
        let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
        let half = 5.0 * s.scale;
        let stroke = s.scale.max(1.0);
        if close {
            line(pm, (cx - half, cy - half), (cx + half, cy + half), stroke, ink);
            line(pm, (cx - half, cy + half), (cx + half, cy - half), stroke, ink);
        } else {
            line(pm, (cx - half, cy), (cx + half, cy), stroke, ink);
        }
    }

    // "Saved": a quiet word at the end of the tab row that fades in and out.
    let saved = s.anims.follow(Key::Saved, if s.saved_on { 1.0 } else { 0.0 }, SLOW_FADE);
    if let (Some((text, alert)), true) = (s.saved.clone(), saved > 0.004) {
        let (tw, th) = s.measure(&text, SMALL);
        let icon = if alert { 0 } else { s.px(16.0) };
        let (outer, _) = tab_rects(s);
        let x = width - s.px(MARGIN) - tw - icon;
        let middle = outer.y + outer.h / 2;
        let ink = alpha(if alert { p.warning } else { p.muted }, saved);
        if !alert {
            let (cx, cy) = (x as f32 + s.px(5.0) as f32, middle as f32);
            let u = s.scale;
            let mut pb = PathBuilder::new();
            pb.move_to(cx - 4.0 * u, cy);
            pb.line_to(cx - 1.2 * u, cy + 3.0 * u);
            pb.line_to(cx + 4.2 * u, cy - 3.2 * u);
            if let Some(path) = pb.finish() {
                let stroke = Stroke { width: 1.5 * u, line_cap: LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Stroke::default() };
                pm.stroke_path(&path, &paint(ink), &stroke, Transform::identity(), None);
            }
        }
        s.draw_text(pm, &text, SMALL, x + icon, middle - th / 2, ink);
    }

    // Tabs: a sliding pill over the four.
    let (outer, cells) = tab_rects(s);
    let (x, y, w, h) = rect_f(outer);
    fill_rr(pm, x, y, w, h, h / 2.0, p.ink(0.05));
    if p.high_contrast {
        stroke_rr(pm, x, y, w, h, h / 2.0, 1.0, p.text);
    }
    let tab = current_tab(s);
    let at = s.anims.follow(Key::Slide(Ctl::Tab), tab as f32, Motion::Spring);
    let first = cells[0];
    let pill_x = first.x as f32 + at * first.w as f32;
    let (pw, ph) = (first.w as f32, first.h as f32);
    fill_rr(pm, pill_x, first.y as f32, pw, ph, ph / 2.0, if p.high_contrast { p.accent } else { p.ink(0.11) });
    stroke_rr(pm, pill_x, first.y as f32, pw, ph, ph / 2.0, s.scale.max(1.0).round(), p.line(0.06));
    for (i, cell) in cells.iter().enumerate() {
        let lit = s.anims.follow(Key::HoverPart(Ctl::Tab, i), if s.hover == Some((Ctl::Tab, i)) { 1.0 } else { 0.0 }, QUICK_FADE);
        let color = if i == tab {
            if p.high_contrast { p.on_accent } else { p.text }
        } else {
            mix(p.subtle, p.secondary, lit)
        };
        s.draw_text_centered(pm, TABS[i], Font { px: 13.0, weight: 600 }, *cell, color);
    }
    if s.focus == Some(Ctl::Tab) && s.focus_ring {
        let r = outer.inflate(s.px(3.0));
        let (x, y, w, h) = rect_f(r);
        stroke_rr(pm, x, y, w, h, h / 2.0, (2.0 * s.scale).round().max(2.0), alpha(p.accent, 0.9));
    }

    // A line under the header, once something has scrolled up behind it.
    let divider = s.anims.follow(Key::Divider, if s.scroll > 0.5 { 1.0 } else { 0.0 }, SLOW_FADE);
    if divider > 0.004 {
        ui::fill_rect(pm, IRect::new(0, header - s.px(1.0).max(1), width, s.px(1.0).max(1)), p.line(0.07 * divider));
    }
}

// --- Hit testing ------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    None,
    /// Somewhere the window can be dragged from.
    Caption,
    Control(Ctl, usize),
    Row(Ctl),
    Thumb,
    Track,
}

fn hit(s: &State, x: i32, y: i32) -> Hit {
    for (ctl, r) in caption_rects(s) {
        if r.contains(x, y) {
            return Hit::Control(ctl, 0);
        }
    }
    let header = s.header_h();
    if y < header {
        let (outer, cells) = tab_rects(s);
        if outer.contains(x, y) {
            let i = cells.iter().position(|c| c.contains(x, y) || (x < c.right() && c.x <= x)).unwrap_or(0);
            return Hit::Control(Ctl::Tab, i.min(TABS.len() - 1));
        }
        return Hit::Caption;
    }
    let (width, _) = s.size();
    if x >= width - s.px(14.0) {
        if let Some((ty, th)) = thumb(s) {
            return if (ty..ty + th).contains(&y) { Hit::Thumb } else { Hit::Track };
        }
    }
    for c in s.scene.controls.iter().rev() {
        let r = s.on_screen(c.rect);
        let r = if matches!(c.kind, Kind::Toggle(_)) { r.inflate(s.px(4.0)) } else { r };
        if !r.contains(x, y) {
            continue;
        }
        let part = match &c.kind {
            Kind::Segments { segs, seg_w, .. } => (((x - r.x - s.px(SEG_INSET)).max(0)) / seg_w).min(segs.len() as i32 - 1) as usize,
            Kind::Picker(_) => match picker_spots(s, s.on_screen(c.rect)).iter().position(|spot| spot.contains(x, y)) {
                Some(i) => i,
                None => continue,
            },
            // The "Reset" at the end of a changed shortcut.
            Kind::Recorder(slot) if s.recording != Some(*slot) && !is_default(s, *slot) => {
                let zone = s.measure("Reset", SMALL).0 + s.px(24.0);
                usize::from(x >= r.right() - zone)
            }
            _ => 0,
        };
        return Hit::Control(c.ctl, part);
    }
    for row in &s.scene.rows {
        if s.on_screen(row.rect).contains(x, y) {
            if let Some(ctl) = row.ctl {
                return Hit::Row(ctl);
            }
        }
    }
    Hit::None
}

// --- Acting -----------------------------------------------------------------------------------

/// Makes a change, saves it and puts it into effect, and says so in the header.
fn commit(s: &mut State, change: impl Fn(&mut Settings)) -> Applied {
    change(&mut s.settings);
    s.restage();
    if s.preview {
        return Applied { saved: true, hotkey_problems: Vec::new(), snipping_ok: true };
    }
    let applied = app::update_settings(&change);
    if applied.saved {
        show_saved(s, "Saved", false);
    } else {
        show_saved(s, "Not saved: the settings file has an error", true);
    }
    applied
}

fn show_saved(s: &mut State, text: &str, alert: bool) {
    s.saved = Some((text.to_string(), alert));
    s.saved_on = true;
    if !s.hwnd.is_invalid() {
        unsafe {
            SetTimer(Some(s.hwnd), SAVED_TIMER, if alert { SAVED_MS * 3 } else { SAVED_MS }, None);
        }
    }
    animate(s);
}

/// Starts the frame timer, if it isn't running.
fn animate(s: &mut State) {
    if !s.hwnd.is_invalid() && !s.preview {
        unsafe {
            SetTimer(Some(s.hwnd), ANIM_TIMER, FRAME_MS, None);
        }
    }
    s.invalidate();
}

fn scroll_by(s: &mut State, delta: f32) {
    s.pinned_tab = None;
    scroll_toward(s, s.scroll_to + delta);
}

fn scroll_toward(s: &mut State, to: f32) {
    s.scroll_to = to.clamp(0.0, s.max_scroll()).round();
    if s.anims.still {
        s.scroll = s.scroll_to;
    }
    wake_bar(s);
    animate(s);
}

fn wake_bar(s: &mut State) {
    BAR_AWAKE.with(|b| b.set(true));
    s.anims.follow(Key::Bar, 1.0, SLOW_FADE);
    if !s.hwnd.is_invalid() {
        unsafe {
            SetTimer(Some(s.hwnd), BAR_TIMER, BAR_IDLE_MS, None);
        }
    }
}

fn jump_to_tab(s: &mut State, tab: usize) {
    let top = s.scene.tab_tops[tab.min(TABS.len() - 1)] - s.px(14.0);
    scroll_toward(s, top as f32);
    s.pinned_tab = Some(tab);
}

/// Scrolls so `r` (page coordinates) is in view.
fn reveal(s: &mut State, r: IRect) {
    let margin = s.px(24.0) as f32;
    let view = s.view_h() as f32;
    let (top, bottom) = (r.y as f32 - margin, r.bottom() as f32 + margin);
    if top < s.scroll_to {
        scroll_toward(s, top);
    } else if bottom > s.scroll_to + view {
        scroll_toward(s, bottom - view);
    }
}

fn activate(s: &mut State, ctl: Ctl, part: usize) {
    match ctl {
        Ctl::Tab => jump_to_tab(s, part),
        Ctl::Minimize => unsafe {
            let _ = ShowWindow(s.hwnd, SW_MINIMIZE);
        },
        Ctl::Close => unsafe {
            let _ = PostMessageW(Some(s.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        Ctl::Autostart => {
            if s.preview {
                s.autostart = !s.autostart;
            } else {
                crate::system::set_autostart(!s.autostart);
                // Shows what really happened, in case the registry said no.
                s.autostart = crate::system::autostart_enabled();
                show_saved(s, "Saved", false);
            }
            s.restage();
        }
        Ctl::Snipping => {
            let on = !s.settings.replace_snipping;
            let applied = commit(s, |settings| settings.replace_snipping = on);
            s.snipping_failed = on && !applied.snipping_ok;
            // Print Screen is the fourth shortcut Prism registers.
            s.print_screen_problem = applied.hotkey_problems.iter().find(|(at, _)| *at == 3).map(|(_, problem)| {
                format!("{problem}. Sign out and back in once, then try again, so Windows lets go of it.")
            });
            s.restage();
        }
        Ctl::Recorder(slot) if part == 1 => {
            stop_recording(s);
            use_shortcut(s, slot, default_shortcut(slot));
        }
        Ctl::Recorder(slot) => start_recording(s, slot),
        Ctl::ToolKey(i) => start_tool_listening(s, i),
        Ctl::ResetToolKeys => {
            stop_tool_listening(s);
            s.tool_note = None;
            if s.settings.tool_keys != ToolKeys::default() {
                commit(s, |settings| settings.tool_keys = ToolKeys::default());
            }
            s.restage();
        }
        Ctl::Magnifier => {
            let on = !s.settings.magnifier;
            commit(s, |settings| settings.magnifier = on);
        }
        Ctl::Cursor => {
            let on = !s.settings.capture_cursor;
            commit(s, |settings| settings.capture_cursor = on);
        }
        Ctl::SaveCopies => {
            let on = !s.settings.save_copies;
            commit(s, |settings| settings.save_copies = on);
        }
        Ctl::Notify => {
            let on = !s.settings.notify;
            commit(s, |settings| settings.notify = on);
        }
        Ctl::QuickFinish => {
            let quick = QUICK[part.min(QUICK.len() - 1)].1;
            if quick != s.settings.quick_finish {
                commit(s, |settings| settings.quick_finish = quick);
            }
        }
        Ctl::StartColor => {
            let color = part.checked_sub(1).and_then(|i| color_choices(&s.settings).get(i).copied());
            if color != s.settings.start_color {
                commit(s, |settings| settings.start_color = color);
            }
        }
        Ctl::StartWidth => {
            let width = part.checked_sub(1).and_then(|i| width_choices(&s.settings).get(i).copied());
            if width != s.settings.start_width {
                commit(s, |settings| settings.start_width = width);
            }
        }
        Ctl::AutoTool => {
            let tool = auto_tool_of(part);
            if tool != s.settings.auto_tool {
                commit(s, |settings| settings.auto_tool = tool);
            }
        }
        Ctl::FinishKey => {
            let key = FINISH_KEYS[part.min(FINISH_KEYS.len() - 1)].1;
            if key != s.settings.finish_key {
                commit(s, |settings| settings.finish_key = key);
            }
        }
        Ctl::EnterAction => {
            let action = ENTER[part.min(ENTER.len() - 1)].1;
            if action != s.settings.enter_action {
                commit(s, |settings| settings.enter_action = action);
            }
        }
        Ctl::PopupPosition => {
            let place = POPUP_POSITIONS[part.min(POPUP_POSITIONS.len() - 1)].1;
            if place != s.settings.popup_position {
                commit(s, |settings| settings.popup_position = place);
                if !s.preview {
                    // Shows where they'll appear now.
                    crate::toast::show("Popups appear here", "Like this one, after you copy or save.", None, None, crate::toast::Click::None);
                }
            }
        }
        Ctl::Folder => begin_edit(s, Field::Folder, None),
        Ctl::FileName => begin_edit(s, Field::FileName, None),
        Ctl::Browse => {
            commit_text(s);
            let start = if s.preview { crate::util::default_save_dir() } else { app::current_save_dir() };
            if let Some(dir) = crate::system::pick_folder(s.hwnd, &start) {
                let dir = dir.display().to_string();
                commit(s, |settings| settings.save_dir = dir.clone());
            }
        }
        Ctl::OpenFolder => {
            commit_text(s);
            let dir = app::current_save_dir();
            let _ = std::fs::create_dir_all(&dir);
            crate::util::shell_open(&dir);
        }
        Ctl::Chip(i) => {
            let placeholder = CHIPS[i.min(CHIPS.len() - 1)].1;
            if s.editing != Some(Field::FileName) {
                begin_edit(s, Field::FileName, None);
                unsafe {
                    SendMessageW(s.edit, EM_SETSEL, Some(WPARAM(usize::MAX >> 1)), Some(LPARAM(-1)));
                }
            }
            let text = wide(placeholder);
            unsafe {
                SendMessageW(s.edit, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(text.as_ptr() as isize)));
            }
        }
    }
}

// --- Text boxes -------------------------------------------------------------------------------

fn field_value(s: &State, field: Field) -> String {
    match field {
        Field::Folder => s.settings.save_dir.clone(),
        Field::FileName => s.settings.file_name.clone(),
    }
}

fn field_ctl(field: Field) -> Ctl {
    match field {
        Field::Folder => Ctl::Folder,
        Field::FileName => Ctl::FileName,
    }
}

fn edit_text(edit: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(edit).max(0) as usize;
        let mut buffer = vec![0u16; len + 1];
        let copied = GetWindowTextW(edit, &mut buffer).max(0) as usize;
        String::from_utf16_lossy(&buffer[..copied])
    }
}

/// Opens a text box over a field, with the caret where it was clicked or everything selected.
fn begin_edit(s: &mut State, field: Field, click_x: Option<i32>) {
    if s.editing == Some(field) {
        return;
    }
    end_edit(s, true);
    stop_recording(s);
    stop_tool_listening(s);
    let Some(rect) = s.control(field_ctl(field)).map(|c| c.rect) else { return };
    reveal(s, rect);
    s.scroll = s.scroll_to;
    s.editing = Some(field);
    s.focus = Some(field_ctl(field));
    let value = wide(&field_value(s, field));
    let hint = wide(&match field {
        Field::Folder => crate::util::default_save_dir().display().to_string(),
        Field::FileName => Settings::default().file_name,
    });
    unsafe {
        let _ = SetWindowTextW(s.edit, PCWSTR(value.as_ptr()));
        SendMessageW(s.edit, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(hint.as_ptr() as isize)));
    }
    place_edit(s);
    unsafe {
        let _ = ShowWindow(s.edit, SW_SHOW);
        let _ = SetFocus(Some(s.edit));
        match click_x {
            Some(x) => {
                let mut client = RECT::default();
                let _ = GetWindowRect(s.edit, &mut client);
                let mut origin = POINT { x: client.left, y: client.top };
                let _ = ScreenToClient(s.hwnd, &mut origin);
                let local = (x - origin.x).max(0) as u32;
                let found = SendMessageW(s.edit, EM_CHARFROMPOS, None, Some(LPARAM((local | (2 << 16)) as isize))).0 as u32 & 0xFFFF;
                SendMessageW(s.edit, EM_SETSEL, Some(WPARAM(found as usize)), Some(LPARAM(found as isize)));
            }
            None => {
                SendMessageW(s.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            }
        }
    }
    s.invalidate();
}

/// Puts the text box over its field, or ends the edit if the field has scrolled out of view.
fn place_edit(s: &mut State) {
    let Some(field) = s.editing else { return };
    let Some(rect) = s.control(field_ctl(field)).map(|c| s.on_screen(c.rect)) else { return };
    let (_, height) = s.size();
    if rect.y < s.header_h() || rect.bottom() > height {
        end_edit(s, true);
        return;
    }
    let pad = s.px(12.0);
    let line = s.line_h(BODY) + s.px(2.0);
    unsafe {
        let _ = SetWindowPos(s.edit, None, rect.x + pad, rect.y + (rect.h - line) / 2, rect.w - 2 * pad, line, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Closes the text box, saving what was typed unless `keep` is false.
fn end_edit(s: &mut State, keep: bool) {
    let Some(field) = s.editing else { return };
    if keep {
        commit_text(s);
    }
    s.editing = None;
    unsafe {
        let _ = KillTimer(Some(s.hwnd), TEXT_TIMER);
        let _ = ShowWindow(s.edit, SW_HIDE);
        if GetFocus() == s.edit {
            let _ = SetFocus(Some(s.hwnd));
        }
    }
    s.focus = Some(field_ctl(field));
    s.restage();
}

/// Saves the text box as typed, if it changed.
fn commit_text(s: &mut State) {
    let Some(field) = s.editing else { return };
    unsafe {
        let _ = KillTimer(Some(s.hwnd), TEXT_TIMER);
    }
    let typed = edit_text(s.edit);
    match field {
        Field::Folder => {
            let typed = typed.trim().to_string();
            if typed != s.settings.save_dir {
                commit(s, |settings| settings.save_dir = typed.clone());
            }
        }
        Field::FileName => {
            if typed != s.settings.file_name {
                commit(s, |settings| settings.file_name = typed.clone());
            }
        }
    }
}

// --- Recording shortcuts ----------------------------------------------------------------------

fn is_repeat(lparam: LPARAM) -> bool {
    lparam.0 & (1 << 30) != 0
}

fn held_now() -> Held {
    let down = |key: u16| unsafe { GetKeyState(key as i32) } < 0;
    Held { ctrl: down(VK_CONTROL.0), alt: down(VK_MENU.0), shift: down(VK_SHIFT.0), win: down(VK_LWIN.0) || down(VK_RWIN.0) }
}

fn held_text(held: Held) -> String {
    [(held.ctrl, "Ctrl"), (held.alt, "Alt"), (held.shift, "Shift"), (held.win, "Win")].iter().filter(|(on, _)| *on).map(|(_, name)| *name).collect::<Vec<_>>().join("+")
}

fn is_modifier(vk: u16) -> bool {
    matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

fn note_held(s: &mut State) {
    let held = held_text(held_now());
    if held != s.held {
        s.held = held;
        s.invalidate();
    }
}

fn start_recording(s: &mut State, slot: usize) {
    if s.recording == Some(slot) {
        return;
    }
    end_edit(s, true);
    stop_tool_listening(s);
    stop_recording(s);
    s.recording = Some(slot);
    s.slot_notes[slot] = None;
    s.held.clear();
    s.focus = Some(Ctl::Recorder(slot));
    if !s.preview {
        app::pause_shortcuts(true);
    }
    s.restage();
}

fn stop_recording(s: &mut State) {
    if s.recording.take().is_none() {
        return;
    }
    s.held.clear();
    if !s.preview {
        app::pause_shortcuts(false);
    }
    s.restage();
}

/// Keys pressed while a shortcut field was listening.
fn keys_pressed(s: &mut State, slot: usize, vk: u16) {
    let held = held_now();
    let plain = held == Held::default();
    if vk == VK_ESCAPE.0 && plain {
        stop_recording(s);
        s.swallow_keys = true;
    } else if vk == VK_BACK.0 && plain {
        use_shortcut(s, slot, default_shortcut(slot));
    } else if vk == VK_DELETE.0 && plain {
        use_shortcut(s, slot, String::new());
    } else if is_modifier(vk) {
        note_held(s);
    } else {
        match hotkey::recorded(held, vk as u32) {
            Ok(text) => use_shortcut(s, slot, text),
            Err(problem) => {
                // Still listening, for another try.
                s.slot_notes[slot] = Some(problem);
                stop_recording_quietly(s, slot);
            }
        }
    }
}

/// Shows a problem under a shortcut while it keeps listening.
fn stop_recording_quietly(s: &mut State, slot: usize) {
    // The row shows the listening note while recording, so the problem would be hidden: stop
    // listening, and show it.
    if s.recording == Some(slot) {
        stop_recording(s);
        s.swallow_keys = true;
    }
    s.restage();
}

/// Puts a recorded shortcut into effect, or says why it can't be used. One taken by another app
/// leaves the old one working.
fn use_shortcut(s: &mut State, slot: usize, text: String) {
    if slot != 0 && s.settings.replace_snipping && prism_core::settings::is_print_screen(&text) {
        s.slot_notes[slot] = Some("Print Screen captures a region while Replace Snipping Tool is on. Click to try another.".into());
        stop_recording_quietly(s, slot);
        return;
    }
    if !text.is_empty() {
        let clash =(0..SHORTCUTS.len()).find(|other| *other != slot && hotkey::same(shortcut(&s.settings.hotkeys, *other), &text));
        if let Some(other) = clash {
            s.slot_notes[slot] = Some(format!("{text} is already set for \u{201C}{}\u{201D}. Click to try another.", SHORTCUTS[other].0));
            stop_recording_quietly(s, slot);
            return;
        }
    }
    let old = shortcut(&s.settings.hotkeys, slot).to_string();
    stop_recording(s);
    s.swallow_keys = true;
    if old != text {
        let applied = commit(s, |settings| set_shortcut(&mut settings.hotkeys, slot, &text));
        if let Some((_, problem)) = applied.hotkey_problems.iter().find(|(at, _)| *at == slot) {
            let problem = problem.clone();
            commit(s, |settings| set_shortcut(&mut settings.hotkeys, slot, &old));
            let kept = if old.is_empty() { "nothing changed".to_string() } else { format!("{old} still works") };
            s.slot_notes[slot] = Some(format!("{problem}, so {kept}. Click to try another."));
        }
    }
    s.restage();
}

/// Whether a message belongs to the keys that just finished a recording: what they'd type, their
/// repeats and their release. Otherwise Alt+letter beeps and Alt alone opens the window menu.
fn swallow_after_recording(s: &mut State, message: u32, lparam: LPARAM) -> bool {
    match message {
        WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR => true,
        WM_KEYDOWN | WM_SYSKEYDOWN if is_repeat(lparam) => true,
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            s.swallow_keys = false;
            false
        }
        WM_KEYUP | WM_SYSKEYUP => {
            if held_now() == Held::default() {
                s.swallow_keys = false;
            }
            true
        }
        _ => false,
    }
}

fn start_tool_listening(s: &mut State, i: usize) {
    end_edit(s, true);
    stop_recording(s);
    s.tool_listening = Some(i);
    s.tool_note = None;
    s.focus = Some(Ctl::ToolKey(i));
    s.restage();
}

fn stop_tool_listening(s: &mut State) {
    if s.tool_listening.take().is_some() {
        s.restage();
    }
}

fn tool_key_pressed(s: &mut State, i: usize, vk: u16) {
    let tool = Tool::ALL[i];
    let held = held_now();
    if vk == VK_ESCAPE.0 {
        stop_tool_listening(s);
        s.swallow_keys = true;
        return;
    }
    if is_modifier(vk) {
        return;
    }
    let letter = if vk == VK_BACK.0 { Some(tool.default_key()) } else { char::from_u32(vk as u32).filter(|c| c.is_ascii_uppercase()) };
    match letter {
        Some(letter) if !held.ctrl && !held.alt && !held.win && ToolKeys::usable(letter) => {
            stop_tool_listening(s);
            s.swallow_keys = true;
            if letter != s.settings.tool_keys.key(tool) {
                let mut keys = s.settings.tool_keys.clone();
                keys.set(tool, letter);
                commit(s, |settings| settings.tool_keys = keys.clone());
            }
        }
        Some(letter) if ToolKeys::usable(letter) => {
            s.tool_note = Some("Just the letter, without Ctrl, Alt or Win.".to_string());
            s.restage();
        }
        Some(letter) => {
            s.tool_note = Some(format!("{letter} is kept for something else: C copies the colour, F takes the whole screen and O copies text. Try another."));
            s.restage();
        }
        None => {
            s.tool_note = Some("Tools take a letter from A to Z. Try another.".to_string());
            s.restage();
        }
    }
}

// --- The keyboard -----------------------------------------------------------------------------

/// Everything the keyboard can reach, in order.
fn focus_order(s: &State) -> Vec<Ctl> {
    let mut order = vec![Ctl::Tab];
    for c in &s.scene.controls {
        if !order.contains(&c.ctl) {
            order.push(c.ctl);
        }
    }
    order
}

fn move_focus(s: &mut State, back: bool) {
    let order = focus_order(s);
    let at = s.focus.and_then(|f| order.iter().position(|c| *c == f));
    let next = match (at, back) {
        (None, false) => 0,
        (None, true) => order.len() - 1,
        (Some(i), false) => (i + 1) % order.len(),
        (Some(i), true) => (i + order.len() - 1) % order.len(),
    };
    s.focus = Some(order[next]);
    s.focus_ring = true;
    if let Some(r) = s.control(order[next]).map(|c| c.rect) {
        reveal(s, r);
    }
    s.invalidate();
}

/// Left and Right on a set of choices.
fn step_choice(s: &mut State, ctl: Ctl, by: i32) {
    if ctl == Ctl::Tab {
        let tab = (current_tab(s) as i32 + by).clamp(0, TABS.len() as i32 - 1) as usize;
        jump_to_tab(s, tab);
        return;
    }
    let Some(c) = s.control(ctl).cloned() else { return };
    let (current, parts) = match &c.kind {
        Kind::Segments { selected, segs, .. } => (*selected, segs.len()),
        Kind::Picker(spot) => (*spot, POPUP_POSITIONS.len()),
        _ => return,
    };
    let next = (current as i32 + by).clamp(0, parts as i32 - 1) as usize;
    if next != current {
        activate(s, ctl, next);
    }
}

fn on_key(s: &mut State, vk: u16) -> bool {
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let line = s.px(48.0) as f32;
    match vk {
        v if v == VK_TAB.0 && ctrl => {
            let tab = (current_tab(s) + if shift { TABS.len() - 1 } else { 1 }) % TABS.len();
            jump_to_tab(s, tab);
        }
        v if v == VK_TAB.0 => move_focus(s, shift),
        v if v == VK_ESCAPE.0 => unsafe {
            let _ = PostMessageW(Some(s.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        v if v == VK_SPACE.0 || v == VK_RETURN.0 => {
            if let Some(ctl) = s.focus {
                s.focus_ring = true;
                match s.control(ctl).map(|c| c.kind.clone()) {
                    Some(Kind::Segments { .. } | Kind::Picker(_)) => {}
                    _ if ctl == Ctl::Tab => {}
                    _ => activate(s, ctl, 0),
                }
            }
        }
        v if v == VK_LEFT.0 || v == VK_RIGHT.0 => {
            let by = if v == VK_LEFT.0 { -1 } else { 1 };
            if let Some(ctl) = s.focus {
                s.focus_ring = true;
                step_choice(s, ctl, by);
            }
        }
        v if v == VK_UP.0 => scroll_by(s, -line),
        v if v == VK_DOWN.0 => scroll_by(s, line),
        v if v == VK_PRIOR.0 => scroll_by(s, -(s.view_h() as f32 * 0.85)),
        v if v == VK_NEXT.0 => scroll_by(s, s.view_h() as f32 * 0.85),
        v if v == VK_HOME.0 => scroll_by(s, -1e9),
        v if v == VK_END.0 => scroll_by(s, 1e9),
        _ => return false,
    }
    true
}

// --- The mouse --------------------------------------------------------------------------------

fn xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

fn on_mouse_move(s: &mut State, x: i32, y: i32) {
    if !s.tracking_leave {
        let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: s.hwnd, dwHoverTime: 0 };
        unsafe {
            let _ = TrackMouseEvent(&mut track);
        }
        s.tracking_leave = true;
    }
    if let Some(grab) = s.thumb_grab {
        if let Some((_, th)) = thumb(s) {
            let (_, height) = s.size();
            let inset = s.px(8.0);
            let track = (height - s.header_h() - 2 * inset - th).max(1) as f32;
            let at = (y - grab - s.header_h() - inset) as f32 / track;
            s.pinned_tab = None;
            s.scroll_to = (at * s.max_scroll()).clamp(0.0, s.max_scroll()).round();
            s.scroll = s.scroll_to;
            place_edit(s);
            wake_bar(s);
            s.invalidate();
        }
        return;
    }
    let found = hit(s, x, y);
    let (width, _) = s.size();
    let near = y > s.header_h() && x >= width - s.px(14.0) && s.max_scroll() > 0.0;
    if near != s.near_bar {
        s.near_bar = near;
        if near {
            wake_bar(s);
        }
        animate(s);
    }
    let hover = match found {
        Hit::Control(ctl, part) => Some((ctl, part)),
        _ => None,
    };
    let row = match found {
        Hit::Row(ctl) => Some(ctl),
        Hit::Control(ctl, _) if s.scene.rows.iter().any(|r| r.ctl == Some(ctl)) => Some(ctl),
        _ => None,
    };
    if row != s.hover_row {
        s.hover_row = row;
        animate(s);
    }
    if hover != s.hover {
        // Some descriptions say what's under the pointer.
        let describes = |h: Option<(Ctl, usize)>| matches!(h, Some((Ctl::AutoTool | Ctl::PopupPosition | Ctl::StartColor | Ctl::StartWidth, _)));
        if describes(hover) || describes(s.hover) {
            s.scene_stale = true;
        }
        s.hover = hover;
        animate(s);
    }
}

fn on_mouse_down(s: &mut State, x: i32, y: i32) {
    s.focus_ring = false;
    let found = hit(s, x, y);
    // A click anywhere else stops listening for keys, and closes the text box.
    let keeps = |ctl: Ctl| matches!(found, Hit::Control(c, _) if c == ctl);
    if let Some(slot) = s.recording {
        if !keeps(Ctl::Recorder(slot)) {
            stop_recording(s);
        }
    }
    if let Some(i) = s.tool_listening {
        if !keeps(Ctl::ToolKey(i)) {
            stop_tool_listening(s);
            s.tool_note = None;
        }
    }
    if let Some(field) = s.editing {
        let chip = matches!(found, Hit::Control(Ctl::Chip(_), _)) && field == Field::FileName;
        if !keeps(field_ctl(field)) && !chip {
            end_edit(s, true);
        }
    }
    match found {
        Hit::Thumb => {
            if let Some((ty, _)) = thumb(s) {
                s.thumb_grab = Some(y - ty);
                unsafe {
                    SetCapture(s.hwnd);
                }
            }
        }
        Hit::Track => {
            let page = s.view_h() as f32 * 0.85;
            let above = thumb(s).is_some_and(|(ty, _)| y < ty);
            scroll_by(s, if above { -page } else { page });
        }
        Hit::Control(ctl @ (Ctl::Folder | Ctl::FileName), _) => {
            let field = if ctl == Ctl::Folder { Field::Folder } else { Field::FileName };
            begin_edit(s, field, Some(x));
        }
        Hit::Control(ctl, part) => {
            s.pressed = Some((ctl, part));
            if !matches!(ctl, Ctl::Chip(_)) || s.editing.is_none() {
                unsafe {
                    let _ = SetFocus(Some(s.hwnd));
                }
            }
            if !matches!(ctl, Ctl::Minimize | Ctl::Close | Ctl::Tab) {
                s.focus = Some(ctl);
            }
            unsafe {
                SetCapture(s.hwnd);
            }
            animate(s);
        }
        Hit::Row(ctl) => {
            let toggles = s.scene.rows.iter().any(|r| r.ctl == Some(ctl) && r.toggles);
            unsafe {
                let _ = SetFocus(Some(s.hwnd));
            }
            if toggles {
                s.pressed = Some((ctl, 0));
                s.focus = Some(ctl);
                unsafe {
                    SetCapture(s.hwnd);
                }
            }
            animate(s);
        }
        Hit::Caption | Hit::None => unsafe {
            let _ = SetFocus(Some(s.hwnd));
        },
    }
}

fn on_mouse_up(s: &mut State, x: i32, y: i32) {
    // Taken before letting go of the mouse: that sends WM_CAPTURECHANGED, which clears them.
    let (grab, pressed) = (s.thumb_grab.take(), s.pressed.take());
    unsafe {
        let _ = ReleaseCapture();
    }
    if grab.is_some() {
        animate(s);
        return;
    }
    let Some((ctl, part)) = pressed else { return };
    let found = hit(s, x, y);
    let same = match found {
        Hit::Control(c, p) => c == ctl && (p == part || matches!(c, Ctl::Tab)),
        Hit::Row(c) => c == ctl,
        _ => false,
    };
    // Choices take where the button came up, so a slide along them still lands.
    let part = match found {
        Hit::Control(c, p) if c == ctl => p,
        _ => part,
    };
    animate(s);
    if same {
        activate(s, ctl, part);
    }
}

// --- The window -------------------------------------------------------------------------------

fn create_edit_font(s: &State) -> HFONT {
    let face = wide(ui_face());
    unsafe {
        CreateFontW(
            -(BODY.px * s.scale).round() as i32,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
            PCWSTR(face.as_ptr()),
        )
    }
}

/// The text box's background: the field's fill over the card over the window.
fn edit_background(p: &Palette) -> Rgba {
    over(over(p.bg, p.ink(0.04 * 0.8)), p.ink(0.09))
}

fn apply_palette(s: &mut State) {
    s.palette = Palette::current();
    s.anims.still = !motion_allowed();
    unsafe {
        if !s.edit_brush.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(s.edit_brush.0));
        }
        s.edit_brush = CreateSolidBrush(colorref(edit_background(&s.palette)));
        if !s.hwnd.is_invalid() {
            let dark: BOOL = s.palette.dark.into();
            let _ = DwmSetWindowAttribute(s.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, std::mem::size_of::<BOOL>() as u32);
            if !s.palette.high_contrast {
                let caption = colorref(s.palette.bg);
                let border = colorref(over(s.palette.bg, s.palette.ink(0.12)));
                let _ = DwmSetWindowAttribute(s.hwnd, DWMWA_CAPTION_COLOR, &caption as *const _ as *const _, 4);
                let _ = DwmSetWindowAttribute(s.hwnd, DWMWA_BORDER_COLOR, &border as *const _ as *const _, 4);
            }
        }
    }
    s.restage();
}

fn apply_scale(s: &mut State, dpi: u32) {
    s.scale = dpi as f32 / 96.0;
    unsafe {
        if !s.edit_font.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(s.edit_font.0));
        }
        s.edit_font = create_edit_font(s);
        if !s.edit.is_invalid() {
            SendMessageW(s.edit, WM_SETFONT, Some(WPARAM(s.edit_font.0 as usize)), Some(LPARAM(1)));
            SendMessageW(s.edit, EM_SETMARGINS, Some(WPARAM(3)), Some(LPARAM(0)));
        }
    }
    s.restage();
}

fn resize_canvas(s: &mut State) {
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(s.hwnd, &mut client);
    }
    let (w, h) = (client.right - client.left, client.bottom - client.top);
    if s.canvas.as_ref().is_some_and(|c| c.w == w && c.h == h) {
        return;
    }
    s.canvas = Canvas::new(w, h);
    s.restage();
    place_edit(s);
}

fn on_paint(s: &mut State) {
    let painting = Instant::now();
    let moving = s.anims.tick();
    // Scrolling glides toward where it's headed.
    let mut scrolling = false;
    let gap = s.scroll_to - s.scroll;
    if gap.abs() > 0.5 {
        let dt = 1.0 / 60.0;
        s.scroll += gap * (1.0 - (-dt * 16.0f32).exp());
        scrolling = true;
    } else {
        s.scroll = s.scroll_to;
    }
    if scrolling {
        place_edit(s);
    }
    render(s);
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(s.hwnd, &mut ps);
        if let Some(c) = &s.canvas {
            let _ = GdiFlush();
            let _ = BitBlt(dc, 0, 0, c.w, c.h, Some(c.dc), 0, 0, SRCCOPY);
        }
        crate::util::log(&format!("settings: painted in {:?}", painting.elapsed()));
        let _ = EndPaint(s.hwnd, &ps);
        if moving || scrolling || s.anims.busy() {
            SetTimer(Some(s.hwnd), ANIM_TIMER, FRAME_MS, None);
        } else {
            let _ = KillTimer(Some(s.hwnd), ANIM_TIMER);
            s.anims.settle();
        }
    }
}

unsafe extern "system" fn settings_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        // The whole window is ours to draw: no frame, but Windows' shadow, corners and snapping.
        if message == WM_NCCALCSIZE && wparam.0 != 0 {
            return LRESULT(0);
        }
        let Some(s) = state(hwnd) else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        match message {
            WM_NCACTIVATE => return DefWindowProcW(hwnd, message, wparam, LPARAM(-1)),
            WM_NCHITTEST => {
                let mut point = POINT { x: xy(lparam).0, y: xy(lparam).1 };
                let _ = ScreenToClient(hwnd, &mut point);
                return LRESULT(if hit(s, point.x, point.y) == Hit::Caption { HTCAPTION as isize } else { HTCLIENT as isize });
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                on_paint(s);
                return LRESULT(0);
            }
            WM_SIZE => resize_canvas(s),
            WM_TIMER => {
                match wparam.0 {
                    ANIM_TIMER => s.invalidate(),
                    TEXT_TIMER => commit_text(s),
                    SAVED_TIMER => {
                        let _ = KillTimer(Some(hwnd), SAVED_TIMER);
                        // The text stays while it fades.
                        s.saved_on = false;
                        animate(s);
                    }
                    BAR_TIMER => {
                        let _ = KillTimer(Some(hwnd), BAR_TIMER);
                        BAR_AWAKE.with(|b| b.set(false));
                        animate(s);
                    }
                    _ => {}
                }
                return LRESULT(0);
            }
            WM_MOUSEMOVE => {
                let (x, y) = xy(lparam);
                on_mouse_move(s, x, y);
                return LRESULT(0);
            }
            WM_MOUSELEAVE => {
                s.tracking_leave = false;
                if s.thumb_grab.is_none() {
                    s.hover = None;
                    s.hover_row = None;
                    s.near_bar = false;
                    s.scene_stale = true;
                    animate(s);
                }
                return LRESULT(0);
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = xy(lparam);
                on_mouse_down(s, x, y);
                return LRESULT(0);
            }
            WM_LBUTTONUP => {
                let (x, y) = xy(lparam);
                on_mouse_up(s, x, y);
                return LRESULT(0);
            }
            WM_CAPTURECHANGED => {
                if s.thumb_grab.is_some() || s.pressed.is_some() {
                    s.thumb_grab = None;
                    s.pressed = None;
                    animate(s);
                }
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32;
                let mut lines = 3u32;
                let _ = SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut lines as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
                let step = s.px(22.0) as f32 * lines.clamp(1, 10) as f32;
                scroll_by(s, -delta / 120.0 * step);
                return LRESULT(0);
            }
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP | WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR
                if s.swallow_keys && swallow_after_recording(s, message, lparam) =>
            {
                return LRESULT(0);
            }
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP if s.recording.is_some() => {
                let slot = s.recording.unwrap_or(0);
                let key = wparam.0 as u16;
                let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
                if down {
                    // Print Screen only ever arrives as a release; see below.
                    if !is_repeat(lparam) && key != VK_SNAPSHOT.0 {
                        keys_pressed(s, slot, key);
                    }
                } else if key == VK_SNAPSHOT.0 {
                    keys_pressed(s, slot, key);
                } else {
                    note_held(s);
                }
                return LRESULT(0);
            }
            WM_CHAR | WM_SYSCHAR if s.recording.is_some() || s.tool_listening.is_some() => return LRESULT(0),
            WM_KEYDOWN | WM_SYSKEYDOWN if s.tool_listening.is_some() => {
                if !is_repeat(lparam) {
                    let i = s.tool_listening.unwrap_or(0);
                    tool_key_pressed(s, i, wparam.0 as u16);
                }
                return LRESULT(0);
            }
            WM_KEYDOWN => {
                if on_key(s, wparam.0 as u16) {
                    return LRESULT(0);
                }
            }
            WM_CHAR => return LRESULT(0),
            WM_COMMAND => {
                let code = (wparam.0 >> 16) as u32;
                if HWND(lparam.0 as *mut _) == s.edit && code == EN_CHANGE && s.editing.is_some() {
                    SetTimer(Some(hwnd), TEXT_TIMER, TEXT_SAVE_DELAY_MS, None);
                    if s.editing == Some(Field::FileName) {
                        // The example under it follows the typing.
                        s.settings.file_name = edit_text(s.edit);
                        s.invalidate();
                    }
                }
                return LRESULT(0);
            }
            WM_CTLCOLOREDIT => {
                let dc = HDC(wparam.0 as *mut _);
                SetTextColor(dc, colorref(s.palette.text));
                SetBkColor(dc, colorref(edit_background(&s.palette)));
                return LRESULT(s.edit_brush.0 as isize);
            }
            WM_SETFOCUS => {
                if s.editing.is_some() {
                    // Back from another window, into the box being typed in.
                    let _ = SetFocus(Some(s.edit));
                }
            }
            WM_KILLFOCUS => {
                if HWND(wparam.0 as *mut _) != s.edit {
                    stop_recording(s);
                    stop_tool_listening(s);
                }
            }
            WM_DPICHANGED => {
                apply_scale(s, (wparam.0 & 0xFFFF) as u32);
                let r = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                resize_canvas(s);
                return LRESULT(0);
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE => {
                apply_palette(s);
            }
            WM_CLOSE => {
                stop_recording(s);
                end_edit(s, true);
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }
            WM_DESTROY => {
                CURRENT.with(|c| {
                    if c.get() == Some(hwnd) {
                        c.set(None);
                    }
                });
                let pointer = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut State;
                if !pointer.is_null() {
                    let owned = Box::from_raw(pointer);
                    // Closing while listening still gives Prism its shortcuts back.
                    if owned.recording.is_some() && !owned.preview {
                        app::pause_shortcuts(false);
                    }
                    let _ = DeleteObject(HGDIOBJ(owned.edit_font.0));
                    let _ = DeleteObject(HGDIOBJ(owned.edit_brush.0));
                }
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

fn register_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(settings_proc),
            hInstance: instance.into(),
            lpszClassName: w!("PrismSettings"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(instance.into()), PCWSTR(1 as _)).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassExW(&class);
    });
}

/// The DPI and work area of the screen to open on: the pointer's.
fn screen_to_open_on(preview: bool) -> (u32, Option<RECT>) {
    if preview {
        return (unsafe { GetDpiForSystem() }, None);
    }
    unsafe {
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let work = GetMonitorInfoW(monitor, &mut info).as_bool().then_some(info.rcWork);
        (dpi_x, work)
    }
}

/// Opens Settings, or brings it forward if it's already open.
pub fn open(_main: HWND, _page: usize) {
    if let Some(existing) = CURRENT.with(|c| c.get()) {
        unsafe {
            let _ = ShowWindow(existing, SW_RESTORE);
            crate::util::force_foreground(existing);
        }
        return;
    }
    let started = Instant::now();
    let Some(settings) = app::settings_snapshot() else { return };
    if let Some(hwnd) = build(settings, false) {
        crate::util::log(&format!("settings: built in {:?}", started.elapsed()));
        CURRENT.with(|c| c.set(Some(hwnd)));
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            crate::util::force_foreground(hwnd);
        }
        crate::util::log(&format!("settings: shown in {:?}", started.elapsed()));
    }
}

/// Closes Settings, saving anything still being typed.
pub fn close() {
    if let Some(hwnd) = CURRENT.with(|c| c.get()) {
        unsafe {
            SendMessageW(hwnd, WM_CLOSE, None, None);
        }
    }
}

/// For the message loop: keys for the text box that mean leaving it, which it would only beep at.
pub fn eats_key(message: &MSG) -> bool {
    let Some(hwnd) = CURRENT.with(|c| c.get()) else { return false };
    let Some(s) = state(hwnd) else { return false };
    if message.hwnd != s.edit || s.edit.is_invalid() {
        return false;
    }
    if message.message != WM_KEYDOWN {
        return false;
    }
    let key = message.wParam.0 as u16;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    match key {
        k if k == VK_TAB.0 => {
            end_edit(s, true);
            unsafe {
                let _ = SetFocus(Some(hwnd));
            }
            move_focus(s, shift);
            true
        }
        k if k == VK_RETURN.0 => {
            end_edit(s, true);
            s.focus_ring = true;
            true
        }
        k if k == VK_ESCAPE.0 => {
            // Leaves the box as it was saved last.
            end_edit(s, false);
            s.focus_ring = true;
            true
        }
        _ => false,
    }
}

fn build(settings: Settings, preview: bool) -> Option<HWND> {
    register_class();
    let (dpi, work) = screen_to_open_on(preview);
    let scale = dpi as f32 / 96.0;
    let mut width = (WIDTH * scale).round() as i32;
    let mut height = (HEIGHT * scale).round() as i32;
    let (mut x, mut y) = (-32000, -32000);
    if let Some(work) = work {
        width = width.min(work.right - work.left - (16.0 * scale) as i32);
        height = height.min(work.bottom - work.top - (16.0 * scale) as i32);
        x = work.left + (work.right - work.left - width) / 2;
        y = work.top + (work.bottom - work.top - height) / 2;
    }
    let title = wide(&format!("{} Settings", crate::util::APP_NAME));
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
    let ex = if preview { WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE } else { WINDOW_EX_STYLE(0) };
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let hwnd = CreateWindowExW(ex, w!("PrismSettings"), PCWSTR(title.as_ptr()), style, x, y, width, height, None, None, Some(instance.into()), None).ok()?;
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            PCWSTR::null(),
            WS_CHILD | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            0,
            0,
            10,
            10,
            Some(hwnd),
            None,
            Some(instance.into()),
            None,
        )
        .unwrap_or_default();
        let margins = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        // Opens and closes at once, without Windows' zoom and fade, which take a fifth of a second.
        let instant = BOOL(1);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, &instant as *const _ as *const _, std::mem::size_of::<BOOL>() as u32);
        let owned = Box::new(State {
            hwnd,
            scale: GetDpiForWindow(hwnd).max(96) as f32 / 96.0,
            palette: Palette::current(),
            font: GdiText::with_face(ui_face()),
            canvas: None,
            settings,
            autostart: if preview { false } else { crate::system::autostart_enabled() },
            snipping_failed: false,
            scene: Scene::default(),
            scene_stale: true,
            anims: Anims::default(),
            scroll: 0.0,
            scroll_to: 0.0,
            pinned_tab: None,
            hover: None,
            hover_row: None,
            pressed: None,
            thumb_grab: None,
            near_bar: false,
            tracking_leave: false,
            focus: None,
            focus_ring: false,
            recording: None,
            held: String::new(),
            swallow_keys: false,
            slot_notes: Default::default(),
            print_screen_problem: None,
            tool_listening: None,
            tool_note: None,
            saved: None,
            saved_on: false,
            editing: None,
            edit,
            edit_font: HFONT::default(),
            edit_brush: HBRUSH::default(),
            preview,
        });
        let pointer = Box::into_raw(owned);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as isize);
        let s = &mut *pointer;
        if preview {
            s.scale = scale;
        }
        apply_scale(s, (s.scale * 96.0).round() as u32);
        apply_palette(s);
        // Frame change, so the whole window becomes client area.
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        resize_canvas(s);
        render(s);
        Some(hwnd)
    }
}

// --- Developer preview ------------------------------------------------------------------------

/// `prism.exe --preview-settings <tab>[-recording|-problem|-saved|-end|-light|-hover] <out.png>`:
/// draws Settings off-screen to a PNG and exits. Nothing is saved and nothing global is touched:
/// no hotkeys, hook, tray icon or registry writes. The settings shown are read from the settings
/// file, or the defaults if there isn't one.
pub fn preview(what: &str, out: &Path) -> i32 {
    let mut parts = what.split('-');
    let tab_key = parts.next().unwrap_or("general");
    let extras: Vec<&str> = parts.collect();
    let Some(tab) = TAB_KEYS.iter().position(|key| *key == tab_key) else { return 2 };
    let path = crate::util::data_dir().join("settings.json");
    let text = std::fs::read_to_string(path).ok();
    let settings = match Settings::load(text.as_deref()) {
        Loaded::Ok(settings) | Loaded::Missing(settings) | Loaded::Invalid(settings, _) => settings,
    };
    let Some(hwnd) = build(settings, true) else { return 3 };
    let Some(s) = state(hwnd) else { return 3 };
    if extras.contains(&"light") {
        s.palette = LIGHT;
    }
    s.anims.still = true;
    render(s);
    jump_to_tab(s, tab);
    s.scroll = s.scroll_to;
    for extra in &extras {
        match *extra {
            "recording" => start_recording(s, 0),
            "problem" => s.slot_notes[0] = Some("Ctrl+Alt+S is already used by another app, so Ctrl+Alt+Shift+S still works. Click to try another.".into()),
            "saved" => {
                s.saved = Some(("Saved".into(), false));
                s.saved_on = true;
            }
            "end" => {
                s.pinned_tab = None;
                s.scroll_to = s.max_scroll();
                s.scroll = s.scroll_to;
            }
            "hover" => {
                // The first row of the tab, lit.
                if let Some(row) = s.scene.rows.iter().find(|r| r.rect.y >= s.scene.tab_tops[tab]) {
                    s.hover_row = row.ctl;
                }
                s.near_bar = true;
                BAR_AWAKE.with(|b| b.set(true));
            }
            // "move744x222" and "click374x84": the pointer, at 100% scale from the window's corner.
            other => {
                let at = |rest: &str| {
                    let (x, y) = rest.split_once('x')?;
                    Some((s.px(x.parse().ok()?), s.px(y.parse().ok()?)))
                };
                if let Some(by) = other.strip_prefix("scroll").and_then(|n| n.parse::<f32>().ok()) {
                    render(s);
                    s.pinned_tab = None;
                    s.scroll_to = (s.scroll_to + by * s.scale).clamp(0.0, s.max_scroll());
                    s.scroll = s.scroll_to;
                } else if let Some((x, y)) = other.strip_prefix("move").and_then(at) {
                    render(s);
                    on_mouse_move(s, x, y);
                } else if let Some((x, y)) = other.strip_prefix("click").and_then(at) {
                    render(s);
                    on_mouse_move(s, x, y);
                    on_mouse_down(s, x, y);
                    on_mouse_up(s, x, y);
                    s.scroll = s.scroll_to;
                }
            }
        }
    }
    s.scene_stale = true;
    render(s);
    if extras.contains(&"saved") || extras.contains(&"hover") {
        // Fades start where they're headed in the preview.
        let keys: Vec<Key> = s.anims.map.keys().copied().collect();
        for key in keys {
            if let Some(a) = s.anims.map.get_mut(&key) {
                a.value = a.target;
            }
        }
        render(s);
    }
    if let Some(dump) = std::env::var_os("PRISM_DUMP") {
        let mut text = format!("scroll {} max {} height {}\n", s.scroll, s.max_scroll(), s.scene.height);
        for c in &s.scene.cards {
            text += &format!("card {c:?}\n");
        }
        for r in &s.scene.rows {
            text += &format!("row {:?} {}\n", r.rect, r.label);
        }
        let _ = std::fs::write(dump, text);
    }
    let Some(canvas) = &s.canvas else { return 4 };
    let mut pixels = unsafe { std::slice::from_raw_parts(canvas.bits, (canvas.w * canvas.h * 4) as usize).to_vec() };
    pixels.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    let (w, h) = (canvas.w as u32, canvas.h as u32);
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    let Some(image) = prism_core::Image::from_bgra(w, h, pixels) else { return 6 };
    let Ok(png) = prism_core::image::encode_png(&image) else { return 6 };
    if std::fs::write(out, png).is_err() {
        return 7;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn every_shortcut_field_reads_and_writes_its_own_setting() {
        let mut keys = Hotkeys::default();
        for slot in 0..SHORTCUTS.len() {
            set_shortcut(&mut keys, slot, &format!("Ctrl+F{}", slot + 1));
        }
        assert_eq!((keys.region.as_str(), keys.screen.as_str(), keys.text.as_str()), ("Ctrl+F1", "Ctrl+F2", "Ctrl+F3"));
        for slot in 0..SHORTCUTS.len() {
            assert_eq!(shortcut(&keys, slot), format!("Ctrl+F{}", slot + 1));
        }
        assert_eq!(default_shortcut(0), "Ctrl+Alt+S");
        assert_eq!(default_shortcut(1), "Shift+PrintScreen");
    }

    #[test]
    fn tab_keys_match_the_tabs() {
        assert_eq!(TAB_KEYS.len(), TABS.len());
        for (key, name) in TAB_KEYS.iter().zip(TABS) {
            assert_eq!(*key, name.to_lowercase());
        }
    }

    #[test]
    fn the_first_tool_choice_means_none() {
        assert_eq!(auto_tool_of(0), None);
        assert_eq!(auto_tool_choice(None), 0);
        for tool in Tool::ALL.into_iter().filter(|t| *t != Tool::Select) {
            assert_eq!(auto_tool_of(auto_tool_choice(Some(tool))), Some(tool));
        }
    }

    #[test]
    fn springs_and_fades_arrive() {
        let mut anims = Anims::default();
        anims.follow(Key::Divider, 0.0, SLOW_FADE);
        anims.follow(Key::Slide(Ctl::Tab), 0.0, Motion::Spring);
        anims.follow(Key::Divider, 1.0, SLOW_FADE);
        anims.follow(Key::Slide(Ctl::Tab), 3.0, Motion::Spring);
        let mut overshot = false;
        for _ in 0..200 {
            anims.last = Some(Instant::now() - Duration::from_millis(16));
            anims.tick();
            overshot |= anims.value(Key::Slide(Ctl::Tab)) > 3.0;
        }
        assert!((anims.value(Key::Divider) - 1.0).abs() < 0.001);
        assert!((anims.value(Key::Slide(Ctl::Tab)) - 3.0).abs() < 0.01);
        assert!(overshot, "the spring overshoots a little, like ease-out-back");
    }
}
