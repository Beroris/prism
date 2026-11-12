//! Settings: one JSON file. Unknown fields are ignored and missing fields take their defaults,
//! so older and newer files both load.

use crate::color::Rgba;
use crate::doc::{Style, Tool, ToolKeys};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EnterAction {
    /// Copy the image to the clipboard.
    #[default]
    Copy,
    /// Save it to the screenshot folder.
    Save,
    /// Both.
    CopyAndSave,
}

/// Which key finishes a capture, doing `enter_action`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FinishKey {
    /// Enter finishes and Esc cancels.
    #[default]
    Enter,
    /// Esc finishes once a region is chosen; before that it still cancels. Enter does nothing.
    Escape,
    /// Either key finishes.
    Both,
}

/// When choosing a region finishes the capture at once, skipping the toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuickFinish {
    /// Never: the toolbar always opens.
    Off,
    /// When Shift is held as the mouse button is let go.
    #[default]
    Shift,
    Ctrl,
    Alt,
    /// Every time: the toolbar never opens, so there's no marking up.
    Always,
}

/// Which corner, or edge middle, of the screen popups appear in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PopupPosition {
    #[default]
    BottomRight,
    BottomCenter,
    BottomLeft,
    TopRight,
    TopCenter,
    TopLeft,
}

/// Where the editing toolbar sits around a chosen region. If there's no room there, it moves to
/// the opposite side, then inside the region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolbarPlace {
    #[default]
    BelowRight,
    BelowCenter,
    BelowLeft,
    AboveRight,
    AboveCenter,
    AboveLeft,
    /// Standing up along the left edge.
    Left,
    /// Standing up along the right edge.
    Right,
}

/// How much freehand strokes, from Draw and the highlighter, are smoothed as they're drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Smoothing {
    /// The stroke follows the pointer exactly.
    Off,
    Light,
    #[default]
    Medium,
    Strong,
}

impl Smoothing {
    /// How far along the pointer's path, either way, each point of a stroke is averaged over:
    /// the Gaussian's sigma, in pixels at 100% scale. See `steady.rs`.
    pub fn sigma(self) -> f32 {
        match self {
            Smoothing::Off => 0.0,
            Smoothing::Light => 4.0,
            Smoothing::Medium => 8.0,
            Smoothing::Strong => 13.0,
        }
    }
}

impl FinishKey {
    pub fn enter(self) -> bool {
        matches!(self, FinishKey::Enter | FinishKey::Both)
    }
    pub fn escape(self) -> bool {
        matches!(self, FinishKey::Escape | FinishKey::Both)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    /// Region capture. Clicking without dragging takes the window under the cursor. Print Screen
    /// and Win+Shift+S capture a region too while `replace_snipping` is on, so this is a shortcut
    /// of the user's own on top of those.
    pub region: String,
    /// Only read from older files, where `region` was Print Screen and this was a second
    /// shortcut. `sanitize` moves it into `region`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub region_alt: String,
    /// The whole monitor under the cursor.
    pub screen: String,
    /// Select a region and copy its text.
    pub text: String,
    /// Capture the last region again, straight away.
    pub last: String,
    /// Open the last region again, chosen and ready to mark up.
    pub last_markup: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Hotkeys { region: "Ctrl+Alt+S".into(), region_alt: String::new(), screen: "Shift+PrintScreen".into(), text: "Ctrl+PrintScreen".into(), last: "Ctrl+Alt+O".into(), last_markup: "Ctrl+Alt+Shift+O".into() }
    }
}

/// Whether a shortcut is Print Screen on its own, however it's written.
pub fn is_print_screen(keys: &str) -> bool {
    let k: String = keys.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    matches!(k.as_str(), "printscreen" | "prtsc" | "prtscn" | "printscrn" | "snapshot")
}

impl Hotkeys {
    /// Older files kept Print Screen as the region shortcut and the user's own as a second one.
    /// Print Screen now comes with `replace_snipping`, so the second one becomes the shortcut.
    fn upgrade(&mut self) {
        if is_print_screen(&self.region) || self.region.trim().is_empty() {
            self.region = std::mem::take(&mut self.region_alt);
        }
        self.region_alt.clear();
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub hotkeys: Hotkeys,
    /// Folder for saved captures. Empty means the system's Screenshots folder.
    pub save_dir: String,
    /// File name pattern without extension. See `naming.rs` for the placeholders.
    pub file_name: String,
    /// What Enter does in the overlay. Ctrl+C always copies and Ctrl+S always saves.
    pub enter_action: EnterAction,
    /// Letting go of a new region with this key held finishes straight away, doing
    /// `enter_action`.
    pub quick_finish: QuickFinish,
    /// Which key does `enter_action`: Enter, Esc or both.
    pub finish_key: FinishKey,
    /// A tool to pick up as soon as a region is chosen, so drawing can start straight away.
    /// None keeps Select.
    pub auto_tool: Option<Tool>,
    /// The letter that picks each tool in the editor, where it differs from the default.
    pub tool_keys: ToolKeys,
    /// Also save every copied capture to the folder, so nothing copied is ever lost.
    pub save_copies: bool,
    /// Keep the screen running while a region is chosen, and capture it when you let go.
    /// Off freezes the screen the moment the shortcut is pressed.
    pub live_capture: bool,
    /// The last region captured, in screen coordinates (x, y, width, height), for capturing it
    /// again.
    pub last_region: Option<[i32; 4]>,
    /// Magnifier with the pixel colour next to the cursor while selecting.
    pub magnifier: bool,
    /// Include the mouse pointer in captures.
    pub capture_cursor: bool,
    /// Notification after saving or copying.
    pub notify: bool,
    /// Where on the screen popups appear.
    pub popup_position: PopupPosition,
    /// Where the editing toolbar sits around the region.
    pub toolbar_place: ToolbarPlace,
    /// Last-used tool and style, restored next time.
    pub tool: Tool,
    pub style: Style,
    /// The colour every capture starts with. None carries on with the last one used.
    pub start_color: Option<Rgba>,
    /// The size every capture starts with. None carries on with the last one used.
    pub start_width: Option<f32>,
    /// How much freehand strokes are smoothed.
    pub smoothing: Smoothing,
    pub palette: Vec<Rgba>,
    /// Whether the user was already asked about taking Print Screen over from Snipping Tool,
    /// so they're asked once, not on every start.
    pub printscreen_prompted: bool,
    /// Take Snipping Tool's shortcuts (Print Screen and Win+Shift+S) for this app. Turning it
    /// off gives them back.
    pub replace_snipping: bool,
    /// The welcome has been seen, so it doesn't open again. A settings file from before the
    /// welcome existed counts as seen: only someone new gets it.
    #[serde(default = "seen")]
    pub onboarded: bool,
    /// Look for a new version on start and once a day, download it in the background and offer
    /// to install it.
    pub check_updates: bool,
}

fn seen() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            schema_version: SCHEMA_VERSION,
            hotkeys: Hotkeys::default(),
            save_dir: String::new(),
            file_name: "Screenshot {date} {time}".into(),
            enter_action: EnterAction::Copy,
            quick_finish: QuickFinish::Shift,
            finish_key: FinishKey::Enter,
            auto_tool: None,
            tool_keys: ToolKeys::default(),
            save_copies: false,
            live_capture: false,
            last_region: None,
            magnifier: true,
            capture_cursor: false,
            notify: true,
            popup_position: PopupPosition::BottomRight,
            toolbar_place: ToolbarPlace::BelowRight,
            tool: Tool::Rect,
            style: Style::default(),
            start_color: None,
            start_width: None,
            smoothing: Smoothing::Medium,
            palette: Rgba::DEFAULT_PALETTE.to_vec(),
            printscreen_prompted: false,
            replace_snipping: false,
            onboarded: false,
            check_updates: true,
        }
    }
}

/// How loading went, so the host can tell the user instead of silently resetting.
#[derive(Debug, PartialEq)]
pub enum Loaded {
    Ok(Settings),
    /// No file yet: defaults, and the host should write them.
    Missing(Settings),
    /// The file didn't parse. Defaults are used and the file is left alone so nothing the user
    /// wrote is lost; the host should not overwrite it.
    Invalid(Settings, String),
}

impl Settings {
    /// The style a new capture starts with: the last one used, with the chosen colour and size.
    pub fn starting_style(&self) -> Style {
        Style { color: self.start_color.unwrap_or(self.style.color), width: self.start_width.unwrap_or(self.style.width), ..self.style }
    }

    pub fn load(text: Option<&str>) -> Loaded {
        let Some(text) = text else { return Loaded::Missing(Settings::default()) };
        match serde_json::from_str::<Settings>(text) {
            Ok(mut s) => {
                s.sanitize();
                Loaded::Ok(s)
            }
            Err(e) => Loaded::Invalid(Settings::default(), e.to_string()),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Repair values that parse but can't be used.
    fn sanitize(&mut self) {
        if self.palette.is_empty() {
            self.palette = Rgba::DEFAULT_PALETTE.to_vec();
        }
        self.palette.truncate(12);
        if !(1.0..=40.0).contains(&self.style.width) {
            self.style.width = Style::default().width;
        }
        if self.style.color.a == 0 {
            self.style.color.a = 255;
        }
        self.hotkeys.upgrade();
        if self.start_width.is_some_and(|w| !(1.0..=40.0).contains(&w)) {
            self.start_width = None;
        }
        if let Some(c) = &mut self.start_color {
            c.a = 255;
        }
        if self.last_region.is_some_and(|[_, _, w, h]| w < 2 || h < 2) {
            self.last_region = None;
        }
        self.tool_keys.sanitize();
        if self.auto_tool == Some(Tool::Select) {
            self.auto_tool = None;
        }
        if self.file_name.trim().is_empty() {
            self.file_name = Settings::default().file_name;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_round_trip() {
        assert_eq!(Settings::load(None), Loaded::Missing(Settings::default()));
        let s = Settings {
            enter_action: EnterAction::Save,
            finish_key: FinishKey::Both,
            quick_finish: QuickFinish::Always,
            auto_tool: Some(Tool::Draw),
            popup_position: PopupPosition::TopCenter,
            toolbar_place: ToolbarPlace::Left,
            smoothing: Smoothing::Strong,
            ..Settings::default()
        };
        assert_eq!(Settings::load(Some(&s.to_json())), Loaded::Ok(s));
    }

    #[test]
    fn partial_file_takes_defaults() {
        let Loaded::Ok(s) = Settings::load(Some(r#"{"magnifier": false, "future_field": 3, "hotkeys": {"text": "Win+Shift+T"}}"#)) else {
            panic!()
        };
        assert!(!s.magnifier);
        assert_eq!(s.smoothing, Smoothing::Medium, "files from before smoothing get it too");
        assert_eq!(s.hotkeys.text, "Win+Shift+T");
        assert_eq!(s.hotkeys.region, "Ctrl+Alt+S");
    }

    #[test]
    fn old_files_with_the_pen_still_load() {
        let Loaded::Ok(s) = Settings::load(Some(r#"{"tool": "pen", "auto_tool": "pen", "tool_keys": {"pen": "q"}}"#)) else { panic!() };
        assert_eq!(s.tool, Tool::Draw);
        assert_eq!(s.auto_tool, Some(Tool::Draw));
        assert_eq!(s.tool_keys.key(Tool::Draw), 'Q');
    }

    #[test]
    fn old_print_screen_shortcuts_upgrade() {
        // Print Screen as the shortcut, and a second one: the second becomes the shortcut.
        let Loaded::Ok(s) = Settings::load(Some(r#"{"hotkeys": {"region": "PrintScreen", "region_alt": "Ctrl+Shift+X"}}"#)) else { panic!() };
        assert_eq!((s.hotkeys.region.as_str(), s.hotkeys.region_alt.as_str()), ("Ctrl+Shift+X", ""));
        assert!(!s.to_json().contains("region_alt"));
        // Print Screen with the second one off: no shortcut of its own.
        let Loaded::Ok(s) = Settings::load(Some(r#"{"hotkeys": {"region": "PrintScreen", "region_alt": ""}}"#)) else { panic!() };
        assert_eq!(s.hotkeys.region, "");
        // A shortcut of the user's own stays, and a stray second one is dropped.
        let Loaded::Ok(s) = Settings::load(Some(r#"{"hotkeys": {"region": "Win+Shift+A", "region_alt": "Ctrl+Alt+S"}}"#)) else { panic!() };
        assert_eq!(s.hotkeys.region, "Win+Shift+A");
        assert!(is_print_screen(" PrtSc ") && !is_print_screen("Shift+PrintScreen"));
    }

    #[test]
    fn captures_start_with_the_chosen_colour_and_size() {
        let last = Style { color: Rgba::BLUE, width: 9.0, fill: true };
        let mut s = Settings { style: last, ..Settings::default() };
        assert_eq!(s.starting_style(), last);
        s.start_color = Some(Rgba::RED);
        s.start_width = Some(3.0);
        assert_eq!(s.starting_style(), Style { color: Rgba::RED, width: 3.0, fill: true });
        let Loaded::Ok(s) = Settings::load(Some(r##"{"start_color": "#00FF0000", "start_width": 0}"##)) else { panic!() };
        assert_eq!((s.start_color.map(|c| c.a), s.start_width), (Some(255), None));
    }

    #[test]
    fn only_someone_new_gets_the_welcome() {
        assert!(!Settings::default().onboarded);
        let Loaded::Ok(s) = Settings::load(Some(r#"{"magnifier": false}"#)) else { panic!() };
        assert!(s.onboarded, "a file from before the welcome counts as seen");
        let Loaded::Ok(s) = Settings::load(Some(r#"{"onboarded": false}"#)) else { panic!() };
        assert!(!s.onboarded);
    }

    #[test]
    fn invalid_keeps_defaults_and_reports() {
        assert!(matches!(Settings::load(Some("{ nope")), Loaded::Invalid(_, _)));
    }

    #[test]
    fn sanitize_repairs() {
        let Loaded::Ok(s) = Settings::load(Some(r##"{"palette": [], "style": {"color": "#00000000", "width": 0, "fill": false}, "file_name": " "}"##))
        else {
            panic!()
        };
        assert_eq!(s.palette.len(), 8);
        assert_eq!(s.style.width, 4.0);
        assert_eq!(s.style.color.a, 255);
        assert_eq!(s.file_name, Settings::default().file_name);
    }
}
