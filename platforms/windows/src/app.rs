//! The resident part: a hidden window that owns the tray icon and hotkeys, opens the overlay,
//! and finishes captures (clipboard, files, OCR) once the overlay is gone.
//!
//! Idle cost is one thread blocked in `GetMessageW`. Encoding, saving and OCR run on short-lived
//! worker threads that post their result back here.

use crate::overlay::{self, Finished, Mode, Outcome};
use crate::util::{APP_NAME, SendHwnd, take_boxed, wide};
use crate::{capture, clipboard, hotkey, keyhook, settings_win, system, toast, tray};
use prism_core::settings::{EnterAction, Loaded, Settings};
use prism_core::{IRect, Image, naming};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, KillTimer, PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SetTimer, WM_APP,
    WM_CONTEXTMENU, WM_COPYDATA, WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP, WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW,
    WS_POPUP,
};
use windows::core::PCWSTR;

pub const WM_OVERLAY_DONE: u32 = WM_APP + 1;
pub const WM_JOB_DONE: u32 = WM_APP + 2;
pub const WM_TRAY: u32 = WM_APP + 3;
/// Win+Shift+S was pressed (from the keyboard hook).
pub const WM_SNIP_KEY: u32 = WM_APP + 4;
pub const MAIN_CLASS: &str = "PrismMain";
/// Marks our WM_COPYDATA messages.
pub const COPYDATA_TAG: usize = 0x5348_4F54;

const HK_REGION: i32 = 1;
const HK_SCREEN: i32 = 2;
const HK_TEXT: i32 = 3;
const HK_LAST: i32 = 5;
const HK_LAST_MARKUP: i32 = 6;
/// Print Screen on its own, which captures a region while Replace Snipping Tool is on.
const HK_PRINT_SCREEN: i32 = 4;
/// The hotkey ids in the order the settings window lists them: region, screen, text, the last
/// region straight away and to mark up, and then Print Screen, which goes with the Replace Snipping Tool switch.
const HOTKEY_SLOTS: [i32; 6] = [HK_REGION, HK_SCREEN, HK_TEXT, HK_LAST, HK_LAST_MARKUP, HK_PRINT_SCREEN];
/// Where Print Screen comes in `HOTKEY_SLOTS`, for the settings window.
pub const PRINT_SCREEN_SLOT: usize = 5;
/// Windows' own Win+Shift+S and Win+S, held while Explorer leaves them alone: see `take_win_keys`.
const HK_SNIP: i32 = 7;
const HK_SEARCH: i32 = 8;
const WIN_SLOTS: [i32; 2] = [HK_SNIP, HK_SEARCH];
const TRAY_CLICK_TIMER: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Region,
    Screen,
    Text,
    /// The last region again, captured straight away.
    Last,
    /// The last region again, chosen and ready to mark up.
    LastMarkup,
    Settings,
    /// The welcome, again.
    Welcome,
    /// Install the downloaded update, or look for one.
    Update,
    Quit,
}

impl Command {
    pub fn parse(s: &str) -> Option<Command> {
        match s.trim_start_matches('-') {
            "region" => Some(Command::Region),
            "screen" | "fullscreen" => Some(Command::Screen),
            "text" | "ocr" => Some(Command::Text),
            "last" | "again" => Some(Command::Last),
            "last-markup" => Some(Command::LastMarkup),
            "settings" => Some(Command::Settings),
            "welcome" => Some(Command::Welcome),
            "update" => Some(Command::Update),
            "quit" => Some(Command::Quit),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Command::Region => "region",
            Command::Screen => "screen",
            Command::Text => "text",
            Command::Last => "last",
            Command::LastMarkup => "last-markup",
            Command::Settings => "settings",
            Command::Welcome => "welcome",
            Command::Update => "update",
            Command::Quit => "quit",
        }
    }
}

struct App {
    hwnd: HWND,
    settings: Settings,
    path: PathBuf,
    mtime: Option<SystemTime>,
    /// False when the file on disk didn't parse: we never overwrite what the user wrote.
    writable: bool,
    taskbar_created: u32,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// The settings window is listening for a new shortcut, so Prism's own are let go until it's
    /// done. Otherwise pressing Print Screen there would start a capture.
    static PAUSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Replace Snipping Tool was turned off since Prism started. Explorer only notices that it may
    /// have Win+Shift+S and Win+S again when it next starts, so until then Prism holds them and
    /// passes them on to Snipping Tool and Search.
    static HANDED_BACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.borrow_mut().as_mut().map(f))
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl App {
    fn save_settings(&mut self) {
        if !self.writable {
            return;
        }
        toast::set_position(self.settings.popup_position);
        if std::fs::write(&self.path, self.settings.to_json()).is_ok() {
            self.mtime = mtime(&self.path);
        }
    }

    /// Pick up edits made to the settings file since we last read it.
    fn reload_if_changed(&mut self) {
        let now = mtime(&self.path);
        if now == self.mtime {
            return;
        }
        self.mtime = now;
        let text = std::fs::read_to_string(&self.path).ok();
        match Settings::load(text.as_deref()) {
            Loaded::Ok(s) => {
                self.settings = s;
                toast::set_position(self.settings.popup_position);
                self.writable = true;
                let _ = self.register_hotkeys();
            }
            Loaded::Missing(_) => {}
            Loaded::Invalid(_, err) => {
                self.writable = false;
                toast::show("Settings file has an error", &err, None, None, toast::Click::Open(self.path.clone()));
            }
        }
    }

    /// Register all hotkeys; returns the ones that failed, as (id, message). Does nothing while
    /// the settings window is recording a shortcut: they're registered when it's done.
    fn register_hotkeys(&self) -> Vec<(i32, String)> {
        if PAUSED.with(|p| p.get()) {
            return Vec::new();
        }
        let h = &self.settings.hotkeys;
        let print_screen = if uses_print_screen(&self.settings) { "PrintScreen" } else { "" }.to_string();
        let failed = [(HK_REGION, &h.region), (HK_SCREEN, &h.screen), (HK_TEXT, &h.text), (HK_LAST, &h.last), (HK_LAST_MARKUP, &h.last_markup), (HK_PRINT_SCREEN, &print_screen)]
            .into_iter()
            .filter_map(|(id, text)| hotkey::register(self.hwnd, id, text).err().map(|e| (id, e)))
            .collect();
        self.take_win_keys();
        failed
    }

    /// Take Win+Shift+S, and Win+S for Search, while Replace Snipping Tool is on. Explorer is told
    /// to leave them alone, and once it has (from its next start) they're ordinary hotkeys, which
    /// also work over admin windows. Until then the keyboard hook catches Win+Shift+S, everywhere
    /// but over admin windows. Returns false if Win+Shift+S couldn't be taken either way.
    fn take_win_keys(&self) -> bool {
        if PAUSED.with(|p| p.get()) {
            return true;
        }
        let on = self.settings.replace_snipping;
        let handed_back = !on && HANDED_BACK.with(|h| h.get());
        let snip = (on || handed_back) && hotkey::register(self.hwnd, HK_SNIP, "Win+Shift+S").is_ok();
        if !snip {
            hotkey::unregister(self.hwnd, HK_SNIP);
        }
        // While Explorer still has Win+S, this fails and Explorer goes on opening Search itself.
        if (on && self.settings.win_s_search) || handed_back {
            let _ = hotkey::register(self.hwnd, HK_SEARCH, "Win+S");
        } else {
            hotkey::unregister(self.hwnd, HK_SEARCH);
        }
        let hooked = keyhook::set_enabled(on && !snip, SendHwnd::new(self.hwnd), WM_SNIP_KEY);
        !on || snip || hooked
    }

    fn save_dir(&self) -> PathBuf {
        if self.settings.save_dir.trim().is_empty() { crate::util::default_save_dir() } else { PathBuf::from(self.settings.save_dir.trim()) }
    }
}

/// What the settings window shows: the settings as they are now.
pub fn settings_snapshot() -> Option<Settings> {
    with(|a| {
        a.reload_if_changed();
        a.settings.clone()
    })
}

/// How a change from the settings window went.
#[derive(Debug, Default)]
pub struct Applied {
    /// False when the settings file has an error: the change works until Prism quits, but the
    /// file is left as it is so nothing the user wrote is lost.
    pub saved: bool,
    /// Shortcuts that couldn't be registered, by their place in the settings window (region,
    /// second region shortcut, screen, text).
    pub hotkey_problems: Vec<(usize, String)>,
    /// False if Win+Shift+S couldn't be taken from Snipping Tool.
    pub snipping_ok: bool,
}

/// Make one change from the settings window, save it, and put it into effect. Only what
/// changed is re-applied: shortcuts are registered again only if they changed, and Snipping
/// Tool's keys are only touched when that switch moves.
pub fn update_settings(change: impl FnOnce(&mut Settings)) -> Applied {
    let Some((saved, keys_changed, search_changed, snipping)) = with(|a| {
        let before = a.settings.clone();
        change(&mut a.settings);
        let snipping = (before.replace_snipping != a.settings.replace_snipping).then_some(a.settings.replace_snipping);
        if snipping.is_some() {
            // Choosing here counts as being asked, so the first-run question never comes back.
            a.settings.printscreen_prompted = true;
        }
        a.save_settings();
        let keys_changed = before.hotkeys != a.settings.hotkeys;
        if keys_changed {
            tray::set_tip(a.hwnd, WM_TRAY, &tray_tip(&a.settings));
        }
        (a.writable, keys_changed, before.win_s_search != a.settings.win_s_search, snipping)
    }) else {
        return Applied::default();
    };
    let snipping_ok = match snipping {
        Some(on) => apply_replace_snipping(on),
        None => true,
    };
    if search_changed {
        with(|a| a.take_win_keys());
    }
    let failed = if keys_changed || snipping.is_some() { with(|a| a.register_hotkeys()).unwrap_or_default() } else { Vec::new() };
    let hotkey_problems = failed.into_iter().filter_map(|(id, message)| HOTKEY_SLOTS.iter().position(|slot| *slot == id).map(|at| (at, message))).collect();
    Applied { saved, hotkey_problems, snipping_ok }
}

/// Let go of Prism's shortcuts while the settings window records a new one (`true`), and take
/// them back afterwards (`false`).
pub fn pause_shortcuts(paused: bool) {
    PAUSED.with(|p| p.set(paused));
    let Some(hwnd) = with(|a| a.hwnd) else { return };
    if paused {
        release_all(hwnd);
    } else {
        // Takes Win+Shift+S and Win+S back too, or the hook if Explorer still has them.
        let _ = with(|a| a.register_hotkeys());
    }
}

/// Let go of every shortcut and the keyboard hook.
fn release_all(hwnd: HWND) {
    for id in HOTKEY_SLOTS.into_iter().chain(WIN_SLOTS) {
        hotkey::unregister(hwnd, id);
    }
    keyhook::set_enabled(false, SendHwnd::new(hwnd), WM_SNIP_KEY);
}

/// Whether Win+Shift+S is still caught by the keyboard hook rather than registered, because
/// Explorer hasn't restarted since Replace Snipping Tool was turned on. Over admin windows it
/// still opens Snipping Tool until then.
pub fn snip_key_waiting() -> bool {
    keyhook::enabled()
}

/// Where captures are saved now.
pub fn current_save_dir() -> PathBuf {
    with(|a| a.save_dir()).unwrap_or_else(crate::util::default_save_dir)
}

pub fn create_window() -> Option<HWND> {
    unsafe {
        let class = wide(MAIN_CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(None).ok()?.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let title = wide(APP_NAME);
        // A real (never shown) top-level window rather than a message-only one: message-only
        // windows don't get the TaskbarCreated broadcast needed to restore the tray icon.
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(GetModuleHandleW(None).ok()?.into()),
            None,
        )
        .ok()
    }
}

/// Set up state, tray and hotkeys. `first_command` runs once everything is ready.
pub fn start(hwnd: HWND, first_command: Option<Command>, background: bool, updated: bool) {
    let path = crate::util::data_dir().join("settings.json");
    let text = std::fs::read_to_string(&path).ok();
    let (settings, first_run, writable, load_error) = match Settings::load(text.as_deref()) {
        Loaded::Ok(s) => (s, false, true, None),
        Loaded::Missing(s) => (s, true, true, None),
        Loaded::Invalid(s, e) => (s, false, false, Some(e)),
    };
    let taskbar_created = unsafe { RegisterWindowMessageW(PCWSTR(wide("TaskbarCreated").as_ptr())) };
    let mut app = App { hwnd, settings, path: path.clone(), mtime: mtime(&path), writable, taskbar_created };
    toast::set_position(app.settings.popup_position);
    if first_run {
        app.save_settings();
    }
    tray::add(hwnd, WM_TRAY, &tray_tip(&app.settings));
    let failures = app.register_hotkeys();
    let prompted = app.settings.printscreen_prompted;
    // Someone new gets the welcome, which asks about Snipping Tool itself. A command from a
    // launcher runs instead, and the welcome waits for the next ordinary start.
    let welcome = !app.settings.onboarded && first_command.is_none();
    let replace_snipping = app.settings.replace_snipping;
    APP.with(|a| *a.borrow_mut() = Some(app));

    if let Some(e) = load_error {
        toast::show("Settings file has an error", &format!("Using defaults until it's fixed. {e}"), None, None, toast::Click::Open(path));
    }

    let region_failed = failures.iter().any(|(id, _)| *id == HK_REGION);
    let mut reported = false;
    if replace_snipping {
        // Re-assert on every start: Windows updates sometimes reset the Print Screen setting.
        apply_replace_snipping(true);
    } else if welcome {
        if !prompted {
            with(|a| {
                a.settings.printscreen_prompted = true;
                a.save_settings();
            });
        }
    } else if !prompted && system::snipping_owns_printscreen() {
        with(|a| {
            a.settings.printscreen_prompted = true;
            a.save_settings();
        });
        let yes = system::ask(
            hwnd,
            "Windows Snipping Tool is using Print Screen and Win+Shift+S.\n\nLet Prism use both shortcuts instead? You can give them back any time in Settings, under Shortcuts.",
        );
        if yes {
            set_replace_snipping(true);
        }
        reported = region_failed;
    }
    let failures: Vec<String> =
        failures.into_iter().filter(|(id, _)| !(reported && *id == HK_REGION)).map(|(_, m)| m).collect();
    if !failures.is_empty() {
        toast::show("Some shortcuts aren't available", &failures.join("; "), None, None, toast::Click::None);
    } else if updated {
        toast::show(
            &format!("Updated to {APP_NAME} {}", env!("CARGO_PKG_VERSION")),
            "Click to see what's new.",
            None,
            None,
            toast::Click::Open(crate::update::this_version_page()),
        );
    } else if first_run && first_command.is_none() && !background && !welcome {
        let key = with(|a| region_keys(&a.settings)).unwrap_or_default();
        toast::show(&format!("{APP_NAME} is running"), &format!("Press {key} to capture. It lives in the tray."), None, None, toast::Click::None);
    }
    crate::update::start(hwnd);
    if welcome {
        settings_win::open_welcome();
    }
    if let Some(c) = first_command {
        run(c);
    }
}

/// Whether Print Screen on its own needs registering for region capture: it comes with Replace
/// Snipping Tool, unless the region shortcut already is Print Screen.
fn uses_print_screen(s: &Settings) -> bool {
    s.replace_snipping && !prism_core::settings::is_print_screen(&s.hotkeys.region)
}

/// Every shortcut that starts a region capture, the main one first: Win+Shift+S once it's
/// Prism's, as it's the one most people reach for, then Print Screen, then Prism's own.
pub fn region_key_list(s: &Settings) -> Vec<String> {
    let snipping = if s.replace_snipping { "Win+Shift+S" } else { "" };
    let print_screen = if uses_print_screen(s) { "PrintScreen" } else { "" };
    [snipping, print_screen, s.hotkeys.region.trim()].into_iter().filter(|k| !k.is_empty()).map(String::from).collect()
}

/// "Win+Shift+S or PrintScreen or Ctrl+Alt+S".
fn region_keys(s: &Settings) -> String {
    region_key_list(s).join(" or ")
}

fn tray_tip(s: &Settings) -> String {
    format!("{APP_NAME} ({})", region_keys(s))
}

/// Take Snipping Tool's shortcuts (`true`) or hand them back (`false`): the Windows Print
/// Screen setting, and Explorer's Win+S keys, which come with Win+Shift+S (see
/// `App::take_win_keys`). Returns false if Win+Shift+S couldn't be taken.
fn apply_replace_snipping(on: bool) -> bool {
    system::set_snipping_printscreen(!on);
    system::set_explorer_s_keys_off(on);
    if !on {
        HANDED_BACK.with(|h| h.set(true));
    }
    with(|a| {
        let _ = a.register_hotkeys();
        a.take_win_keys()
    })
    .unwrap_or(false)
}

/// The user's choice, from the first-run question.
fn set_replace_snipping(on: bool) {
    let saved = with(|a| {
        a.settings.replace_snipping = on;
        a.settings.printscreen_prompted = true;
        a.save_settings();
    });
    if saved.is_none() {
        return;
    }
    let ok = apply_replace_snipping(on);
    if on && ok {
        toast::show(
            "Snipping Tool's shortcuts are now yours",
            "Print Screen and Win+Shift+S open Prism. Sign out and back in once so they also work over apps running as administrator.",
            None,
            None,
            toast::Click::None,
        );
    } else if on {
        toast::show("Couldn't take over Win+Shift+S", "Print Screen still works. Try again from the tray menu.", None, None, toast::Click::None);
    } else {
        toast::show("Snipping Tool's shortcuts are back", "Print Screen and Win+Shift+S open Snipping Tool again.", None, None, toast::Click::None);
    }
}

pub fn run(cmd: Command) {
    with(|a| a.reload_if_changed());
    let Some((hwnd, s)) = with(|a| (a.hwnd, a.settings.clone())) else { return };
    match cmd {
        Command::Region | Command::Text | Command::LastMarkup => {
            // The last region, if it's still on a screen; otherwise this is an ordinary capture.
            let preset = (cmd == Command::LastMarkup)
                .then_some(s.last_region)
                .flatten()
                .map(|[x, y, w, h]| IRect::new(x, y, w, h).intersect(&capture::virtual_screen()))
                .filter(|r| r.w >= 2 && r.h >= 2);
            toast::close();
            let opts = overlay::Options {
                mode: if cmd == Command::Text { Mode::Text } else { Mode::Capture },
                magnifier: s.magnifier,
                capture_cursor: s.capture_cursor && cmd != Command::Text,
                style: s.starting_style(),
                palette: s.palette.clone(),
                enter: s.enter_action,
                finish_key: s.finish_key,
                quick_finish: s.quick_finish,
                auto_tool: s.auto_tool,
                tool_keys: s.tool_keys.clone(),
                toolbar_place: s.toolbar_place,
                smoothing: s.smoothing,
                live: s.live_capture,
                preset,
            };
            if !overlay::open(hwnd, opts) {
                toast::show("Couldn't capture the screen", "Try again in a moment.", None, None, toast::Click::None);
            }
        }
        Command::Screen => {
            let p = crate::util::cursor_pos();
            let (l, t, r, b) = crate::util::monitor_rect(crate::util::monitor_at(p));
            capture_now(IRect::from_ltrb(l, t, r, b), &s);
        }
        Command::Last => {
            let last = s.last_region.map(|[x, y, w, h]| IRect::new(x, y, w, h).intersect(&capture::virtual_screen())).filter(|r| r.w >= 2 && r.h >= 2);
            match last {
                Some(rect) => capture_now(rect, &s),
                // Nothing captured yet, or it was on a monitor that's gone: choose one.
                None => run(Command::Region),
            }
        }
        Command::Settings => settings_win::open(hwnd, settings_win::PAGE_GENERAL),
        Command::Welcome => settings_win::open_welcome(),
        Command::Update => match crate::update::phase() {
            crate::update::Phase::Ready(_) => crate::update::install(),
            _ => crate::update::check_now(),
        },
        Command::Quit => unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
        },
    }
}

/// Capture `rect` (screen coordinates) with no overlay, and copy or save it as Enter would.
fn capture_now(rect: IRect, s: &Settings) {
    toast::close();
    match capture::grab(rect, s.capture_cursor) {
        Some(img) => {
            let outcome = match s.enter_action {
                EnterAction::Copy => Outcome::Copy(img),
                EnterAction::Save => Outcome::Save(img),
                EnterAction::CopyAndSave => Outcome::CopyAndSave(img),
            };
            finish(Finished { outcome, tool: s.tool, style: s.style, screen_rect: Some(rect) });
        }
        None => toast::show("Couldn't capture the screen", "Try again in a moment.", None, None, toast::Click::None),
    }
}

/// Results from worker threads.
enum Job {
    CopyReady { img: Image, png: Option<Vec<u8>>, thumb: Image, near: Option<IRect>, saved: Option<Result<PathBuf, String>> },
    Saved { result: Result<PathBuf, String>, thumb: Image, near: Option<IRect> },
    Ocr { result: Result<String, String>, near: Option<IRect> },
}

fn save_png(img: &Image, dir: &Path, pattern: &str) -> Result<PathBuf, String> {
    save_encoded(img, None, dir, pattern)
}

/// Save to the folder under a fresh name, reusing `png` if it's already encoded.
fn save_encoded(img: &Image, png: Option<&[u8]>, dir: &Path, pattern: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Can't create {}: {e}", dir.display()))?;
    let base = naming::format_name(pattern, crate::util::now_stamp(), img.w, img.h);
    let path = dir.join(naming::unique_name(&base, "png", |n| dir.join(n).exists()));
    let encoded;
    let bytes = match png {
        Some(b) => b,
        None => {
            encoded = prism_core::image::encode_png(img)?;
            &encoded
        }
    };
    std::fs::write(&path, bytes).map_err(|e| format!("Can't write {}: {e}", path.display()))?;
    Ok(path)
}

fn write_png(img: &Image, path: &Path) -> Result<(), String> {
    let png = prism_core::image::encode_png(img)?;
    std::fs::write(path, png).map_err(|e| format!("Can't write {}: {e}", path.display()))
}

fn spawn(main: SendHwnd, f: impl FnOnce() -> Job + Send + 'static) {
    let _ = std::thread::Builder::new().name("shot-job".into()).spawn(move || {
        let job = f();
        main.post_boxed(WM_JOB_DONE, 0, job);
    });
}

fn finish(done: Finished) {
    // Remember where a capture was taken, for capturing the same region again.
    let region = done.screen_rect.filter(|_| !matches!(done.outcome, Outcome::Cancel | Outcome::Color(_))).map(|r| [r.x, r.y, r.w, r.h]);
    let Some((hwnd, dir, pattern, save_copies)) = with(|a| {
        let new_region = region.is_some() && a.settings.last_region != region;
        if a.settings.style != done.style || a.settings.tool != done.tool || new_region {
            a.settings.style = done.style;
            a.settings.tool = done.tool;
            if new_region {
                a.settings.last_region = region;
            }
            a.save_settings();
        }
        (a.hwnd, a.save_dir(), a.settings.file_name.clone(), a.settings.save_copies)
    }) else {
        return;
    };
    let main = SendHwnd::new(hwnd);
    let near = done.screen_rect;
    match done.outcome {
        Outcome::Cancel => {}
        Outcome::Copy(img) => copy_job(main, img, save_copies, dir, pattern, near),
        Outcome::CopyAndSave(img) => copy_job(main, img, true, dir, pattern, near),
        Outcome::Save(img) => spawn(main, move || {
            let result = save_png(&img, &dir, &pattern);
            Job::Saved { result, thumb: toast::thumbnail(&img, 240, 150), near }
        }),
        Outcome::SaveAs(img) => {
            let name = naming::format_name(&pattern, crate::util::now_stamp(), img.w, img.h);
            if let Some(path) = system::save_dialog(hwnd, &dir, &name) {
                spawn(main, move || {
                    let result = write_png(&img, &path).map(|_| path);
                    Job::Saved { result, thumb: toast::thumbnail(&img, 240, 150), near }
                });
            }
        }
        Outcome::Ocr(img) => spawn(main, move || Job::Ocr { result: crate::ocr::recognize(&img), near }),
        Outcome::Color(c) => {
            let hex = format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b);
            clipboard::set_text(hwnd, &hex);
            notify(&format!("Copied {hex}"), &format!("rgb({}, {}, {})", c.r, c.g, c.b), None, near, toast::Click::None);
        }
    }
}

/// Encode once on a worker; the PNG goes on the clipboard and, if asked, to a file.
fn copy_job(main: SendHwnd, img: Image, save: bool, dir: PathBuf, pattern: String, near: Option<IRect>) {
    spawn(main, move || {
        let png = prism_core::image::encode_png(&img).ok();
        let saved = save.then(|| save_encoded(&img, png.as_deref(), &dir, &pattern));
        let thumb = toast::thumbnail(&img, 240, 150);
        Job::CopyReady { img, png, thumb, near, saved }
    });
}

fn notify(title: &str, body: &str, thumb: Option<&Image>, near: Option<IRect>, click: toast::Click) {
    if with(|a| a.settings.notify).unwrap_or(true) {
        toast::show(title, body, thumb, near, click);
    }
}

fn job_done(job: Job) {
    let Some(hwnd) = with(|a| a.hwnd) else { return };
    match job {
        Job::CopyReady { img, png, thumb, near, saved } => {
            let ok = clipboard::set_image(hwnd, &img, png.as_deref());
            drop(img);
            match (ok, saved) {
                (false, _) => toast::show("Couldn't copy", "Another app is holding the clipboard. Try again.", Some(&thumb), near, toast::Click::None),
                (true, Some(Ok(path))) => notify("Copied and saved · click to show", &file_label(&path), Some(&thumb), near, toast::Click::Reveal(path)),
                (true, Some(Err(e))) => toast::show("Copied, but not saved", &e, Some(&thumb), near, toast::Click::None),
                (true, None) => notify("Copied to clipboard", "Paste it anywhere with Ctrl+V", Some(&thumb), near, toast::Click::None),
            }
        }
        Job::Saved { result: Ok(path), thumb, near } => {
            notify("Saved · click to show", &file_label(&path), Some(&thumb), near, toast::Click::Reveal(path));
        }
        Job::Saved { result: Err(e), thumb, near } => toast::show("Couldn't save", &e, Some(&thumb), near, toast::Click::None),
        Job::Ocr { result: Ok(text), near } if text.trim().is_empty() => {
            toast::show("No text found", "Try a larger selection, or text with more contrast.", None, near, toast::Click::None);
        }
        Job::Ocr { result: Ok(text), near } => {
            if clipboard::set_text(hwnd, &text) {
                let lines = text.lines().count();
                let title = if lines == 1 { "Copied text".to_string() } else { format!("Copied {lines} lines of text") };
                notify(&title, &text, None, near, toast::Click::None);
            } else {
                toast::show("Couldn't copy", "Another app is holding the clipboard. Try again.", None, near, toast::Click::None);
            }
        }
        Job::Ocr { result: Err(e), near } => toast::show("Couldn't read text", &e, None, near, toast::Click::None),
    }
}

fn file_label(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn on_menu() {
    let Some((hwnd, hotkeys)) = with(|a| {
        a.reload_if_changed();
        let s = &a.settings.hotkeys;
        // The menu has room for the main shortcut only.
        (a.hwnd, [region_key_list(&a.settings).into_iter().next().unwrap_or_default(), s.screen.clone(), s.text.clone()])
    }) else {
        return;
    };
    let ready = match crate::update::phase() {
        crate::update::Phase::Ready(v) => Some(v),
        _ => None,
    };
    let Some(choice) = tray::show_menu(hwnd, &hotkeys, ready.as_deref()) else { return };
    use tray::Choice as C;
    match choice {
        // Let the menu fade out before freezing the screen.
        C::Region => start_delayed(Command::Region),
        C::Screen => start_delayed(Command::Screen),
        C::Text => start_delayed(Command::Text),
        C::OpenFolder => {
            if let Some(dir) = with(|a| a.save_dir()) {
                let _ = std::fs::create_dir_all(&dir);
                crate::util::shell_open(&dir);
            }
        }
        C::Settings => settings_win::open(hwnd, settings_win::PAGE_GENERAL),
        C::Quit => run(Command::Quit),
        C::Update => crate::update::install(),
    }
}

thread_local! {
    static PENDING: RefCell<Option<Command>> = const { RefCell::new(None) };
}

/// Runs `cmd` in a moment, once a menu or window that's closing is off the screen.
pub fn run_soon(cmd: Command) {
    start_delayed(cmd);
}

fn start_delayed(cmd: Command) {
    PENDING.with(|p| *p.borrow_mut() = Some(cmd));
    if let Some(hwnd) = with(|a| a.hwnd) {
        unsafe {
            SetTimer(Some(hwnd), TRAY_CLICK_TIMER, 180, None);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_HOTKEY => {
                match wp.0 as i32 {
                    HK_REGION | HK_PRINT_SCREEN => run(Command::Region),
                    HK_SCREEN => run(Command::Screen),
                    HK_TEXT => run(Command::Text),
                    HK_LAST => run(Command::Last),
                    HK_LAST_MARKUP => run(Command::LastMarkup),
                    HK_SNIP if with(|a| a.settings.replace_snipping).unwrap_or(false) => {
                        // Releasing Win after a hotkey can still open the Start menu.
                        keyhook::mask_start_menu();
                        run(Command::Region);
                    }
                    // Replace Snipping Tool was turned off, and Explorer hasn't taken these back yet.
                    HK_SNIP => crate::util::shell_open(Path::new("ms-screenclip:")),
                    HK_SEARCH => keyhook::open_windows_search(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_TRAY => {
                match (lp.0 & 0xFFFF) as u32 {
                    WM_LBUTTONUP => start_delayed(Command::Region),
                    WM_RBUTTONUP | WM_CONTEXTMENU => on_menu(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_TIMER if wp.0 == crate::update::CHECK_TIMER || wp.0 == crate::update::PROMPT_TIMER => {
                crate::update::on_timer(wp.0);
                LRESULT(0)
            }
            crate::update::WM_UPDATE => {
                crate::update::on_message(lp);
                LRESULT(0)
            }
            WM_TIMER if wp.0 == TRAY_CLICK_TIMER => {
                let _ = KillTimer(Some(hwnd), TRAY_CLICK_TIMER);
                if let Some(c) = PENDING.with(|p| p.borrow_mut().take()) {
                    run(c);
                }
                LRESULT(0)
            }
            WM_OVERLAY_DONE => {
                let done: Finished = take_boxed(lp);
                finish(done);
                LRESULT(0)
            }
            WM_JOB_DONE => {
                let job: Job = take_boxed(lp);
                job_done(job);
                LRESULT(0)
            }
            WM_COPYDATA => {
                let cds = &*(lp.0 as *const COPYDATASTRUCT);
                if cds.dwData == COPYDATA_TAG && !cds.lpData.is_null() {
                    let bytes = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
                    let text = String::from_utf8_lossy(bytes).into_owned();
                    // Run after returning, so the sender isn't blocked while the overlay opens.
                    if let Some(cmd) = Command::parse(&text) {
                        PENDING.with(|p| *p.borrow_mut() = Some(cmd));
                        SetTimer(Some(hwnd), TRAY_CLICK_TIMER, 1, None);
                    }
                    return LRESULT(1);
                }
                LRESULT(0)
            }
            WM_SNIP_KEY => {
                let () = take_boxed(lp);
                run(Command::Region);
                LRESULT(0)
            }
            WM_DESTROY => {
                settings_win::close();
                tray::remove(hwnd, WM_TRAY);
                release_all(hwnd);
                toast::close();
                PostQuitMessage(0);
                LRESULT(0)
            }
            m if m != 0 && Some(m) == with(|a| a.taskbar_created) => {
                // Explorer restarted: put the icon back. It has now read whether to leave the
                // Win+S keys alone, so Win+Shift+S may be a hotkey Prism can take at last.
                if let Some(tip) = with(|a| tray_tip(&a.settings)) {
                    tray::add(hwnd, WM_TRAY, &tip);
                }
                with(|a| a.take_win_keys());
                settings_win::refresh();
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
