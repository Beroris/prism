//! The capture overlay: a topmost window over the whole virtual desktop showing the frozen
//! screen. Pick a region (drag) or a window (click), annotate in place, then copy or save.
//!
//! Layers, all the size of the virtual desktop and freed when the overlay closes:
//! - `base`: the capture, untouched.
//! - `dim`: the capture darkened, shown outside the selection.
//! - `committed`: the capture with every finished shape drawn on it.
//! - the back buffer: a DIB the frame is composed into and blitted from.
//!
//! A frame copies `dim`, then the selection from `committed` (or from `scratch`, which adds the
//! shape being drawn), then draws the chrome. Only the area that changed is recomposed.

use crate::capture::{self, WinRect};
use crate::text::GdiText;
use crate::ui::{self, Btn, IconExtra};
use crate::util::{SendHwnd, wide};
use prism_core::doc::{self, Shape, Style, Tool};
use prism_core::image::copy_rect;
use prism_core::render::Canvas;
use prism_core::{Doc, IRect, Image, Pt, Rgba};
use std::cell::RefCell;
use tiny_skia::PixmapMut;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, EndPaint, GdiFlush, HBITMAP, HDC, HGDIOBJ, InvalidateRect, PAINTSTRUCT, SRCCOPY, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN,
    VK_RIGHT, VK_MENU, VK_SHIFT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, HTCLIENT, IDC_ARROW, IDC_CROSS,
    IDC_HAND, IDC_IBEAM, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, KillTimer, LoadCursorW, RegisterClassExW,
    SW_SHOW, SetCursor, SetCursorPos, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, HWND_TOPMOST,
    SWP_NOMOVE, SWP_NOSIZE, WM_CAPTURECHANGED, WM_CHAR, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR};

/// What the overlay was opened for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Capture and annotate.
    Capture,
    /// Pick a region and copy its text; no annotation.
    Text,
}

/// How the overlay ended. Sent to the main window, which does the work after the overlay is
/// gone so closing always feels instant.
pub enum Outcome {
    Cancel,
    Copy(Image),
    Save(Image),
    SaveAs(Image),
    CopyAndSave(Image),
    Ocr(Image),
    Color(Rgba),
}

pub struct Finished {
    pub outcome: Outcome,
    /// Tool and style in use at the end, remembered for next time.
    pub tool: Tool,
    pub style: Style,
    /// Where the selection was, in screen coordinates, so a toast can appear near it.
    pub screen_rect: Option<IRect>,
}

pub struct Options {
    pub mode: Mode,
    pub magnifier: bool,
    pub capture_cursor: bool,
    pub style: Style,
    pub palette: Vec<Rgba>,
    pub enter: prism_core::settings::EnterAction,
    pub finish_key: prism_core::settings::FinishKey,
    pub quick_finish: prism_core::settings::QuickFinish,
    pub auto_tool: Option<Tool>,
    pub tool_keys: prism_core::doc::ToolKeys,
}

const CLASS: &str = "PrismOverlay";
const CARET_TIMER: usize = 1;
/// How much of the colour to keep outside the selection, out of 256.
const DIM_KEEP: u16 = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

const HANDLES: [Handle; 8] = [Handle::NW, Handle::N, Handle::NE, Handle::E, Handle::SE, Handle::S, Handle::SW, Handle::W];

enum Drag {
    None,
    /// Mouse down while choosing a region; becomes a rectangle once it moves.
    NewSel { start: (i32, i32), moved: bool },
    MoveSel { start: (i32, i32), orig: IRect },
    Resize { handle: Handle, orig: IRect },
    Draw { shape: Shape, start: Pt },
    MoveShape { idx: usize, start: Pt, orig: Shape, cur: Shape, lifted: bool },
}

/// Points of a pen stroke drawn live each frame before older ones are baked.
const BAKE_AFTER: usize = 64;

struct Baked {
    /// The selection with the stroke's first `upto` points drawn over it.
    image: Image,
    sel: IRect,
    upto: usize,
}

impl Default for Baked {
    fn default() -> Self {
        Baked { image: Image::new(0, 0), sel: IRect::default(), upto: 0 }
    }
}

/// The part of a freehand stroke from point `k` on, drawn exactly as the whole stroke draws it
/// there: the stroke curves through the midpoints between points, so this part starts at the
/// midpoint before `k`.
fn stroke_from(pts: &[Pt], k: usize) -> Vec<Pt> {
    if k == 0 {
        return pts.to_vec();
    }
    let (a, b) = (pts[k - 1], pts[k]);
    std::iter::once(Pt::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)).chain(pts[k..].iter().copied()).collect()
}

struct TextEdit {
    pos: Pt,
    text: String,
    style: Style,
    /// Set when editing a shape that already exists.
    index: Option<usize>,
}

struct Back {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
}

pub struct Overlay {
    hwnd: HWND,
    main: SendHwnd,
    mode: Mode,
    /// Virtual desktop origin: image (0, 0) is this screen point.
    origin: (i32, i32),
    bounds: IRect,
    base: Image,
    dim: Image,
    committed: Image,
    scratch: Image,
    scratch_live: bool,
    /// The live shape changed: draw `scratch` again before the next paint, not on every mouse
    /// move, so a fast mouse never queues up more drawing than the screen can show.
    scratch_stale: bool,
    /// The start of a long freehand stroke, already drawn over the selection.
    baked: Baked,
    back: Back,
    windows: Vec<WinRect>,
    monitors: Vec<IRect>,
    sel: Option<IRect>,
    doc: Doc,
    tool: Tool,
    style: Style,
    palette: Vec<Rgba>,
    palette_open: bool,
    drag: Drag,
    mouse: (i32, i32),
    hover_window: Option<IRect>,
    hover_btn: Option<Btn>,
    selected_shape: Option<usize>,
    editing: Option<TextEdit>,
    caret_on: bool,
    magnifier: bool,
    enter: prism_core::settings::EnterAction,
    finish_key: prism_core::settings::FinishKey,
    quick_finish: prism_core::settings::QuickFinish,
    /// Picked up when a region is chosen.
    auto_tool: Option<Tool>,
    /// The region before a new one was started from outside it, put back if the new one comes
    /// to nothing.
    reselect_from: Option<IRect>,
    tool_keys: prism_core::doc::ToolKeys,
    text: GdiText,
    /// Everything drawn over plain `dim` in the last frame, so it can be cleared next frame.
    last_bounds: IRect,
}

thread_local! {
    static OPEN: RefCell<Option<SendHwnd>> = const { RefCell::new(None) };
}

/// The overlay window, if one is open.
pub fn current() -> Option<HWND> {
    OPEN.with(|o| o.borrow().map(|h| h.get()))
}

fn register_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let name = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
            lpszClassName: PCWSTR(name.as_ptr()),
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        std::mem::forget(name);
    });
}

/// Capture the screen and open the overlay. Returns false if capture failed.
pub fn open(main: HWND, opts: Options) -> bool {
    if let Some(h) = current() {
        unsafe {
            let _ = SetForegroundWindow(h);
        }
        return true;
    }
    let t0 = std::time::Instant::now();
    let vs = capture::virtual_screen();
    let Some(base) = capture::grab(vs, opts.capture_cursor) else { return false };
    let t_grab = t0.elapsed();
    register_class();
    let origin = (vs.x, vs.y);
    let (w, h) = (base.w, base.h);
    let dim = base.dimmed(DIM_KEEP);
    let committed = base.clone();
    let monitors = capture::monitors(origin);
    unsafe {
        let class = wide(CLASS);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(class.as_ptr()),
            PCWSTR(wide(crate::util::APP_NAME).as_ptr()),
            WS_POPUP,
            vs.x,
            vs.y,
            vs.w,
            vs.h,
            None,
            None,
            Some(GetModuleHandleW(None).unwrap_or_default().into()),
            None,
        ) else {
            return false;
        };
        let disable: BOOL = true.into();
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, &disable as *const _ as *const _, 4);
        let Some(back) = make_back(w as i32, h as i32) else {
            let _ = DestroyWindow(hwnd);
            return false;
        };
        let windows = capture::windows_front_to_back(hwnd, origin);
        let mouse = crate::util::cursor_pos();
        let mut ov = Box::new(Overlay {
            hwnd,
            main: SendHwnd::new(main),
            mode: opts.mode,
            origin,
            bounds: IRect::new(0, 0, w as i32, h as i32),
            base,
            dim,
            committed,
            scratch: Image::new(0, 0),
            scratch_live: false,
            scratch_stale: false,
            baked: Baked::default(),
            back,
            windows,
            monitors,
            sel: None,
            doc: Doc::default(),
            tool: Tool::Select,
            style: opts.style,
            palette: opts.palette,
            palette_open: false,
            drag: Drag::None,
            mouse: (mouse.x - origin.0, mouse.y - origin.1),
            hover_window: None,
            hover_btn: None,
            selected_shape: None,
            editing: None,
            caret_on: true,
            magnifier: opts.magnifier,
            enter: opts.enter,
            finish_key: opts.finish_key,
            quick_finish: opts.quick_finish,
            auto_tool: opts.auto_tool,
            reselect_from: None,
            tool_keys: opts.tool_keys,
            text: GdiText::new(),
            last_bounds: IRect::default(),
        });
        ov.update_hover();
        // Compose the whole first frame before showing, so nothing flashes.
        ov.compose(ov.bounds);
        ov.last_bounds = ov.chrome_bounds();
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(ov) as isize);
        OPEN.with(|o| *o.borrow_mut() = Some(SendHwnd::new(hwnd)));
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        crate::util::force_foreground(hwnd);
        crate::util::log(&format!("overlay: grab {:?}, visible {:?}", t_grab, t0.elapsed()));
    }
    true
}

fn make_back(w: i32, h: i32) -> Option<Back> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        let bmi = BITMAPINFO {
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
        let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return None;
        };
        let old = SelectObject(dc, bmp.into());
        Some(Back { dc, bmp, old, bits: bits as *mut u8 })
    }
}

impl Drop for Back {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

fn key_down(vk: u16) -> bool {
    unsafe { GetKeyState(vk as i32) < 0 }
}

impl Overlay {
    // ---------- geometry ----------

    fn monitor_of(&self, x: i32, y: i32) -> IRect {
        self.monitors.iter().copied().find(|m| m.contains(x, y)).unwrap_or_else(|| {
            // Nearest monitor for points in gaps between monitors.
            self.monitors
                .iter()
                .copied()
                .min_by_key(|m| {
                    let dx = (m.x - x).max(0).max(x - m.right());
                    let dy = (m.y - y).max(0).max(y - m.bottom());
                    dx * dx + dy * dy
                })
                .unwrap_or(self.bounds)
        })
    }

    /// UI scale for the monitor the action is on.
    fn scale(&self) -> f32 {
        let (x, y) = match self.sel {
            Some(s) => (s.x + s.w / 2, s.y + s.h / 2),
            None => self.mouse,
        };
        let m = crate::util::monitor_at(POINT { x: x + self.origin.0, y: y + self.origin.1 });
        crate::util::monitor_scale(m)
    }

    /// The selection as it is right now, including one being dragged out.
    fn live_sel(&self) -> Option<IRect> {
        match &self.drag {
            Drag::NewSel { start, moved: true } => {
                Some(IRect::spanning(start.0, start.1, self.mouse.0, self.mouse.1).intersect(&self.bounds))
            }
            _ => self.sel,
        }
    }

    /// The undimmed area: the selection, or the window that a click would pick.
    fn bright(&self) -> Option<IRect> {
        match &self.drag {
            Drag::NewSel { moved: true, .. } => self.live_sel(),
            Drag::NewSel { moved: false, .. } => self.hover_window.or(self.sel),
            _ => self.sel.or(self.hover_window),
        }
    }

    fn update_hover(&mut self) {
        let (x, y) = self.mouse;
        self.hover_window = if self.sel.is_none() {
            self.windows
                .iter()
                .find(|w| w.rect.contains(x, y))
                .map(|w| w.rect.intersect(&self.monitor_of(x, y)).intersect(&self.bounds))
                .filter(|r| !r.is_empty())
                .or_else(|| Some(self.monitor_of(x, y)))
        } else {
            None
        };
    }

    fn toolbar(&self) -> Option<ui::ToolbarLayout> {
        if self.mode != Mode::Capture {
            return None;
        }
        if matches!(self.drag, Drag::NewSel { .. } | Drag::Resize { .. } | Drag::MoveSel { .. }) {
            return None;
        }
        let sel = self.sel?;
        let mon = self.monitor_of(sel.x + sel.w / 2, sel.bottom().min(self.bounds.bottom() - 1));
        Some(ui::layout_toolbar(sel, mon, self.scale()))
    }

    fn palette_layout(&self, tb: &ui::ToolbarLayout) -> Option<ui::PaletteLayout> {
        if !self.palette_open {
            return None;
        }
        let anchor = tb.buttons.iter().find(|(b, _)| *b == Btn::Color).map(|(_, r)| *r)?;
        let mon = self.monitor_of(anchor.x, anchor.y);
        Some(ui::layout_palette(anchor, tb.above, &self.palette, mon, self.scale()))
    }

    fn handle_at(&self, x: i32, y: i32) -> Option<Handle> {
        let sel = self.sel?;
        if self.mode != Mode::Capture || self.editing.is_some() {
            return None;
        }
        let rects = ui::handle_rects(sel, self.scale());
        let slack = (4.0 * self.scale()) as i32;
        HANDLES.iter().zip(rects.iter()).find(|(_, r)| r.inflate(slack).contains(x, y)).map(|(h, _)| *h)
    }

    fn magnifier_rect(&self) -> Option<IRect> {
        if !self.magnifier {
            return None;
        }
        let show = match self.drag {
            Drag::NewSel { .. } | Drag::Resize { .. } => true,
            Drag::None => self.sel.is_none(),
            _ => false,
        };
        if !show {
            return None;
        }
        let s = self.scale();
        let size = (MAG_CELLS as f32 * MAG_ZOOM * s) as i32;
        let info = (40.0 * s) as i32;
        let (w, h) = (size, size + info);
        let off = (22.0 * s) as i32;
        let (mx, my) = self.mouse;
        let mon = self.monitor_of(mx, my);
        let x = if mx + off + w <= mon.right() { mx + off } else { mx - off - w };
        let y = if my + off + h <= mon.bottom() { my + off } else { my - off - h };
        Some(IRect::new(x, y, w, h))
    }

    fn size_label(&self) -> Option<(IRect, String)> {
        let sel = self.live_sel().or(if self.drag_is_none() { self.hover_window } else { None })?;
        let s = self.scale();
        let label = format!("{} × {}", sel.w, sel.h);
        let (w, h) = ui::pill_size(&self.text, &label, s);
        let gap = (6.0 * s) as i32;
        let y = if sel.y - gap - h >= self.monitor_of(sel.x, sel.y).y { sel.y - gap - h } else { sel.y + gap };
        let x = sel.x.max(self.monitor_of(sel.x, sel.y).x);
        Some((IRect::new(x, y, w, h), label))
    }

    fn drag_is_none(&self) -> bool {
        matches!(self.drag, Drag::None)
    }

    /// Everything that differs from plain `dim` in the current frame.
    fn chrome_bounds(&self) -> IRect {
        let s = self.scale();
        let pad = (14.0 * s) as i32;
        let mut r = IRect::default();
        if let Some(b) = self.bright() {
            r = r.union(&b.inflate(pad));
        }
        if let Some((l, _)) = self.size_label() {
            r = r.union(&l);
        }
        if let Some(m) = self.magnifier_rect() {
            r = r.union(&m.inflate(pad));
        }
        if let Some(tb) = self.toolbar() {
            // Tooltips and the palette can appear on either side; cover a band around it.
            r = r.union(&tb.panel.inflate((70.0 * s) as i32));
            if let Some(p) = self.palette_layout(&tb) {
                r = r.union(&p.panel.inflate(pad));
            }
        }
        if let Some(e) = &self.editing {
            let (w, h) = self.text.measure(&e.text, doc::text_px(e.style.width), false);
            r = r.union(&IRect::new(e.pos.x as i32 - pad, e.pos.y as i32 - pad, w + 2 * pad + 40, h + 2 * pad));
        }
        r.intersect(&self.bounds)
    }

    /// Repaint whatever changed since the last frame.
    fn refresh(&mut self) {
        let now = self.chrome_bounds();
        let dirty = now.union(&self.last_bounds);
        self.last_bounds = now;
        self.invalidate(dirty);
    }

    fn invalidate(&self, r: IRect) {
        if r.is_empty() {
            return;
        }
        let rc = RECT { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() };
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), Some(&rc), false);
        }
    }

    // ---------- layers ----------

    /// Redraw `committed` from scratch, optionally leaving one shape out (it is being moved or
    /// edited and is drawn live instead).
    fn rebuild(&mut self, skip: Option<usize>) {
        self.committed.data.copy_from_slice(&self.base.data);
        let (w, h) = (self.committed.w, self.committed.h);
        if let Some(mut c) = Canvas::new(&mut self.committed.data, w, h, (0, 0)) {
            for (i, s) in self.doc.shapes().iter().enumerate() {
                if Some(i) != skip {
                    c.draw(s, &self.text);
                }
            }
        }
    }

    /// Draw a finished shape onto `committed` and record it.
    fn commit_shape(&mut self, s: Shape) {
        if s.is_degenerate() {
            return;
        }
        let (w, h) = (self.committed.w, self.committed.h);
        if let Some(mut c) = Canvas::new(&mut self.committed.data, w, h, (0, 0)) {
            c.draw(&s, &self.text);
        }
        self.doc.push(s);
    }

    /// The shape currently drawn live (being drawn, moved or typed), if any.
    fn live_shape(&self) -> Option<Shape> {
        match &self.drag {
            Drag::Draw { shape, .. } => return Some(shape.clone()),
            Drag::MoveShape { cur, lifted: true, .. } => return Some(cur.clone()),
            _ => {}
        }
        self.editing.as_ref().map(|e| self.text_shape(e))
    }

    fn text_shape(&self, e: &TextEdit) -> Shape {
        let (w, h) = self.text.measure(&e.text, doc::text_px(e.style.width), false);
        Shape::Text { pos: e.pos, text: e.text.clone(), style: e.style, w: w as f32, h: h as f32 }
    }

    /// Prepare `scratch`: the selection from `committed` plus the live shape.
    fn update_scratch(&mut self) {
        self.scratch_stale = false;
        let (Some(sel), Some(shape)) = (self.sel, self.live_shape()) else {
            self.scratch_live = false;
            return;
        };
        if self.scratch.w != sel.w as u32 || self.scratch.h != sel.h as u32 {
            self.scratch = Image::new(sel.w as u32, sel.h as u32);
        }
        let (w, h) = (self.scratch.w, self.scratch.h);
        // A long stroke: copy what's baked and draw only the newest points.
        let drawing = matches!(self.drag, Drag::Draw { .. });
        if let (true, Shape::Pen { pts, style }) = (drawing, &shape) {
            if let Some(k) = self.bake(sel, pts, *style) {
                self.scratch.data.copy_from_slice(&self.baked.image.data);
                let tail = Shape::Pen { pts: stroke_from(pts, k), style: *style };
                if let Some(mut c) = Canvas::new(&mut self.scratch.data, w, h, (sel.x, sel.y)) {
                    c.draw(&tail, &self.text);
                }
                self.scratch_live = true;
                return;
            }
        }
        copy_rect(&self.committed.data, self.committed.w, sel, &mut self.scratch.data, self.scratch.w, 0, 0);
        if let Some(mut c) = Canvas::new(&mut self.scratch.data, w, h, (sel.x, sel.y)) {
            c.draw(&shape, &self.text);
        }
        self.scratch_live = true;
    }

    /// Draw the older points of a long pen stroke over the selection once, into `baked`, so
    /// each frame draws at most `BAKE_AFTER` points however long the stroke gets. Returns the
    /// point the live part starts at, or None while the stroke is short enough to draw whole.
    ///
    /// Only the pen: it's opaque, so where the baked and live parts overlap looks the same.
    /// Highlighter strokes are translucent and would darken there. When the stroke ends it's
    /// drawn again whole, so what's kept is exact.
    fn bake(&mut self, sel: IRect, pts: &[Pt], style: Style) -> Option<usize> {
        if self.baked.sel != sel || self.baked.upto >= pts.len() {
            // A new selection, or Shift made the stroke straight.
            self.baked.upto = 0;
        }
        let k = self.baked.upto;
        if pts.len() - k <= BAKE_AFTER {
            return (k > 0).then_some(k);
        }
        let upto = pts.len() - BAKE_AFTER / 2;
        if k == 0 {
            self.baked.sel = sel;
            self.baked.image = Image::new(sel.w as u32, sel.h as u32);
            copy_rect(&self.committed.data, self.committed.w, sel, &mut self.baked.image.data, sel.w as u32, 0, 0);
        }
        let chunk = Shape::Pen { pts: stroke_from(&pts[..=upto], k), style };
        let (w, h) = (self.baked.image.w, self.baked.image.h);
        if let Some(mut c) = Canvas::new(&mut self.baked.image.data, w, h, (sel.x, sel.y)) {
            c.draw(&chunk, &self.text);
        }
        self.baked.upto = upto;
        Some(upto)
    }

    // ---------- painting ----------

    fn back_pixmap(&self) -> PixmapMut<'static> {
        let len = self.bounds.w as usize * self.bounds.h as usize * 4;
        // The DIB lives as long as the overlay, and the pixmap never outlives a paint call.
        let slice = unsafe { std::slice::from_raw_parts_mut(self.back.bits, len) };
        PixmapMut::from_bytes(slice, self.bounds.w as u32, self.bounds.h as u32).expect("back buffer")
    }

    fn compose(&mut self, r: IRect) {
        let r = r.intersect(&self.bounds);
        if r.is_empty() {
            return;
        }
        unsafe {
            let _ = GdiFlush();
        }
        let w = self.bounds.w as u32;
        let mut pm = self.back_pixmap();
        let back = pm.data_mut();
        copy_rect(&self.dim.data, w, r, back, w, r.x, r.y);
        if let Some(b) = self.bright() {
            let area = b.intersect(&r);
            if !area.is_empty() {
                let use_scratch = self.scratch_live && self.sel == Some(b);
                if use_scratch {
                    let local = area.offset(-b.x, -b.y);
                    copy_rect(&self.scratch.data, self.scratch.w, local, back, w, area.x, area.y);
                } else {
                    copy_rect(&self.committed.data, w, area, back, w, area.x, area.y);
                }
            }
        }
        self.draw_chrome(&mut pm);
    }

    fn draw_chrome(&self, pm: &mut PixmapMut) {
        let s = self.scale();
        if let Some(b) = self.bright() {
            ui::selection_border(pm, b, s);
            if self.sel.is_some() && self.mode == Mode::Capture && self.editing.is_none() && !matches!(self.drag, Drag::Draw { .. }) {
                if let Some(sel) = self.sel {
                    ui::draw_handles(pm, sel, s);
                }
            }
        }
        if let (Some(i), Some(_)) = (self.selected_shape, self.sel) {
            let shape = match &self.drag {
                Drag::MoveShape { cur, .. } => Some(cur.clone()),
                _ => self.doc.shapes().get(i).map(|s| (**s).clone()),
            };
            if let Some(shape) = shape {
                ui::dashed_rect(pm, shape.bounds().inflate(3.0).to_irect(), ui::ACCENT, s);
            }
        }
        if let Some(e) = &self.editing {
            let (tw, th) = self.text.measure(&e.text, doc::text_px(e.style.width), false);
            let pad = (4.0 * s) as i32;
            let box_ = IRect::new(e.pos.x as i32 - pad, e.pos.y as i32 - pad, tw.max((8.0 * s) as i32) + 2 * pad + 2, th + 2 * pad);
            ui::dashed_rect(pm, box_, Rgba::WHITE, s);
            if self.caret_on {
                // Caret after the last character of the last line.
                let last_line = e.text.rsplit('\n').next().unwrap_or("");
                let (lw, lh) = self.text.measure(last_line, doc::text_px(e.style.width), false);
                let line_count = e.text.split('\n').count() as i32;
                let y = e.pos.y as i32 + lh * (line_count - 1);
                ui::fill_rect(pm, IRect::new(e.pos.x as i32 + lw + 1, y + 1, (1.5 * s).max(1.0) as i32, lh - 2), e.style.color);
            }
        }
        if let Some((r, label)) = self.size_label() {
            ui::pill(pm, &self.text, &label, r.x, r.y, s);
        }
        if let Some(m) = self.magnifier_rect() {
            self.draw_magnifier(pm, m, s);
        }
        if let Some(tb) = self.toolbar() {
            self.draw_toolbar(pm, &tb, s);
        }
        if self.mode == Mode::Text && self.sel.is_none() && self.drag_is_none() {
            self.draw_hint(pm, "Select the text to copy", s);
        }
    }

    fn draw_hint(&self, pm: &mut PixmapMut, msg: &str, s: f32) {
        let mon = self.monitor_of(self.mouse.0, self.mouse.1);
        let (w, _) = ui::pill_size(&self.text, msg, s);
        ui::pill(pm, &self.text, msg, mon.x + (mon.w - w) / 2, mon.y + (24.0 * s) as i32, s);
    }

    fn draw_toolbar(&self, pm: &mut PixmapMut, tb: &ui::ToolbarLayout, s: f32) {
        ui::panel(pm, tb.panel, 9.0 * s, s);
        for sep in &tb.separators {
            ui::fill_rect(pm, *sep, ui::PANEL_BORDER);
        }
        let extra = IconExtra { color: self.style.color, width: self.style.width };
        for (btn, r) in &tb.buttons {
            let active = match btn {
                Btn::Tool(t) => *t == self.tool,
                Btn::Color => self.palette_open,
                _ => false,
            };
            let enabled = match btn {
                Btn::Undo => self.doc.can_undo() || self.editing.is_some(),
                Btn::Redo => self.doc.can_redo(),
                _ => true,
            };
            let inner = r.inflate(-(2.0 * s) as i32);
            if active {
                ui::fill_rrect(pm, inner, 6.0 * s, ui::ACTIVE);
            } else if self.hover_btn == Some(*btn) && enabled {
                ui::fill_rrect(pm, inner, 6.0 * s, ui::HOVER);
            }
            let color = if !enabled {
                ui::FG_DISABLED
            } else if active {
                Rgba::WHITE
            } else if *btn == Btn::Close {
                ui::FG_DIM
            } else {
                ui::FG
            };
            ui::icon(pm, &self.text, *btn, *r, color, s, extra);
        }
        if let Some(p) = self.palette_layout(tb) {
            ui::panel(pm, p.panel, 9.0 * s, s);
            for (c, r) in &p.swatches {
                ui::swatch(pm, *r, *c, *c == self.style.color, s);
            }
            ui::fill_toggle(pm, p.fill, self.style.fill, self.style.color, s);
        }
        if let Some(btn) = self.hover_btn {
            if let Some((_, r)) = tb.buttons.iter().find(|(b, _)| *b == btn) {
                let tip = if btn == Btn::Width { format!("Size {}  scroll wheel", self.style.width as i32) } else { btn.tooltip(&self.tool_keys) };
                let (w, h) = ui::pill_size(&self.text, &tip, s);
                let gap = (8.0 * s) as i32;
                let y = if tb.above { tb.panel.y - gap - h } else { tb.panel.bottom() + gap };
                let mon = self.monitor_of(r.x, r.y);
                let x = (r.x + r.w / 2 - w / 2).clamp(mon.x + 2, mon.right() - w - 2);
                if !self.palette_open {
                    ui::pill(pm, &self.text, &tip, x, y, s);
                }
            }
        }
    }

    fn draw_magnifier(&self, pm: &mut PixmapMut, r: IRect, s: f32) {
        let cell = (MAG_ZOOM * s).round().max(4.0) as i32;
        let grid = MAG_CELLS as i32 * cell;
        let area = IRect::new(r.x, r.y, grid, grid);
        ui::panel(pm, IRect::new(r.x, r.y, r.w, r.h), 8.0 * s, s);
        let half = MAG_CELLS as i32 / 2;
        let (w, h) = (pm.width() as i32, pm.height() as i32);
        {
            let data = pm.data_mut();
            for j in 0..MAG_CELLS as i32 {
                for i in 0..MAG_CELLS as i32 {
                    let (sx, sy) = (self.mouse.0 - half + i, self.mouse.1 - half + j);
                    let (b, g, rr) = match self.base.rgb_at(sx, sy) {
                        Some((r_, g_, b_)) => (b_, g_, r_),
                        None => (20, 20, 24),
                    };
                    for y in 0..cell {
                        let py = area.y + j * cell + y;
                        if py < 0 || py >= h {
                            continue;
                        }
                        for x in 0..cell {
                            let px = area.x + i * cell + x;
                            if px < 0 || px >= w {
                                continue;
                            }
                            let o = ((py * w + px) * 4) as usize;
                            data[o] = b;
                            data[o + 1] = g;
                            data[o + 2] = rr;
                            data[o + 3] = 255;
                        }
                    }
                }
            }
        }
        // Crosshair band and the centre cell.
        let tint = Rgba { r: 0x4C, g: 0x8D, b: 0xFF, a: 0x38 };
        ui::fill_rect(pm, IRect::new(area.x, area.y + half * cell, grid, cell), tint);
        ui::fill_rect(pm, IRect::new(area.x + half * cell, area.y, cell, grid), tint);
        let c = IRect::new(area.x + half * cell, area.y + half * cell, cell, cell);
        ui::stroke_rrect(pm, c.inflate(1), 0.0, 1.0, Rgba::BLACK);
        ui::stroke_rrect(pm, c, 0.0, 1.0, Rgba::WHITE);
        ui::stroke_rrect(pm, area, 0.0, 1.0, ui::PANEL_BORDER);
        let (sx, sy) = (self.mouse.0 + self.origin.0, self.mouse.1 + self.origin.1);
        let color = self.base.rgb_at(self.mouse.0, self.mouse.1).unwrap_or((0, 0, 0));
        let hex = format!("#{:02X}{:02X}{:02X}", color.0, color.1, color.2);
        let ty = area.bottom() + (5.0 * s) as i32;
        let px = 11.5 * s;
        ui::text(pm, &self.text, &format!("{sx}, {sy}"), px, false, r.x + (8.0 * s) as i32, ty, ui::FG_DIM);
        let swatch = IRect::new(r.x + (8.0 * s) as i32, ty + (19.0 * s) as i32, (10.0 * s) as i32, (10.0 * s) as i32);
        ui::fill_rrect(pm, swatch, 2.0 * s, Rgba::rgb(color.0, color.1, color.2));
        ui::stroke_rrect(pm, swatch, 2.0 * s, 1.0, ui::PANEL_BORDER);
        ui::text(pm, &self.text, &hex, px, true, swatch.right() + (6.0 * s) as i32, ty + (15.0 * s) as i32, ui::FG);
        ui::text(pm, &self.text, "C", px, false, r.right() - (18.0 * s) as i32, ty + (15.0 * s) as i32, ui::FG_DIM);
    }

    fn paint(&mut self) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(self.hwnd, &mut ps);
            let rc = ps.rcPaint;
            if self.scratch_stale {
                self.update_scratch();
            }
            let r = IRect::from_ltrb(rc.left, rc.top, rc.right, rc.bottom);
            self.compose(r);
            let _ = BitBlt(dc, r.x, r.y, r.w, r.h, Some(self.back.dc), r.x, r.y, SRCCOPY);
            let _ = EndPaint(self.hwnd, &ps);
        }
    }

    // ---------- finishing ----------

    fn result_image(&mut self) -> Option<Image> {
        self.commit_text();
        let sel = self.sel?;
        Some(self.committed.crop(sel))
    }

    fn finish(&mut self, outcome: Outcome) {
        let screen_rect = self.sel.map(|s| s.offset(self.origin.0, self.origin.1));
        let done = Finished { outcome, tool: self.tool, style: self.style, screen_rect };
        // Close first: the work happens after the overlay is gone.
        unsafe {
            let _ = ShowWindow(self.hwnd, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
        }
        self.main.post_boxed(crate::app::WM_OVERLAY_DONE, 0, done);
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }

    fn act(&mut self, what: Btn) {
        if self.sel.is_none() {
            // Acting before choosing: use the window or monitor under the cursor.
            self.sel = self.hover_window;
        }
        let Some(img) = self.result_image() else { return };
        let out = match what {
            Btn::Copy => Outcome::Copy(img),
            Btn::Save => Outcome::Save(img),
            Btn::Ocr => Outcome::Ocr(img),
            _ => return,
        };
        self.finish(out);
    }

    /// Finish the way Enter does: read the text in text mode, otherwise do `enter`.
    fn confirm(&mut self) {
        if self.mode != Mode::Text {
            return self.act_enter();
        }
        if self.sel.is_none() {
            self.sel = self.hover_window;
        }
        if let Some(sel) = self.sel {
            let img = self.base.crop(sel);
            self.finish(Outcome::Ocr(img));
        }
    }

    /// Whether letting go of a new region should finish at once, as the settings say.
    fn quick_finish_held(&self) -> bool {
        use prism_core::settings::QuickFinish;
        match self.quick_finish {
            QuickFinish::Off => false,
            QuickFinish::Shift => key_down(VK_SHIFT.0),
            QuickFinish::Ctrl => key_down(VK_CONTROL.0),
            QuickFinish::Alt => key_down(VK_MENU.0),
            QuickFinish::Always => true,
        }
    }

    fn act_enter(&mut self) {
        use prism_core::settings::EnterAction;
        if self.sel.is_none() {
            self.sel = self.hover_window;
        }
        let Some(img) = self.result_image() else { return };
        let out = match self.enter {
            EnterAction::Copy => Outcome::Copy(img),
            EnterAction::Save => Outcome::Save(img),
            EnterAction::CopyAndSave => Outcome::CopyAndSave(img),
        };
        self.finish(out);
    }

    // ---------- text editing ----------

    fn commit_text(&mut self) {
        let Some(e) = self.editing.take() else { return };
        unsafe {
            let _ = KillTimer(Some(self.hwnd), CARET_TIMER);
        }
        let shape = self.text_shape(&e);
        match e.index {
            Some(i) if shape.is_degenerate() => {
                self.doc.remove(i);
            }
            Some(i) => self.doc.replace(i, shape),
            None if !shape.is_degenerate() => self.doc.push(shape),
            None => {}
        }
        self.rebuild(None);
        self.scratch_live = false;
    }

    fn start_text(&mut self, p: Pt) {
        // Clicking an existing text shape edits it.
        let tol = 3.0 * self.scale();
        if let Some(i) = self.doc.hit(p, tol) {
            if let Shape::Text { pos, text, style, .. } = &*self.doc.shapes()[i] {
                self.editing = Some(TextEdit { pos: *pos, text: text.clone(), style: *style, index: Some(i) });
                self.rebuild(Some(i));
                self.after_text_change();
                return;
            }
        }
        // Place the click at the middle of the first line, which is where people aim.
        let (_, lh) = self.text.measure("", doc::text_px(self.style.width), false);
        self.editing = Some(TextEdit { pos: Pt::new(p.x, p.y - lh as f32 / 2.0), text: String::new(), style: self.style, index: None });
        self.after_text_change();
    }

    fn after_text_change(&mut self) {
        self.caret_on = true;
        unsafe {
            SetTimer(Some(self.hwnd), CARET_TIMER, 530, None);
        }
        self.update_scratch();
        self.refresh();
    }

    // ---------- input ----------

    fn on_mouse_down(&mut self, x: i32, y: i32) {
        self.mouse = (x, y);
        let p = Pt::new(x as f32, y as f32);
        unsafe {
            SetCapture(self.hwnd);
        }
        // Toolbar and palette first.
        if let Some(tb) = self.toolbar() {
            if let Some(pl) = self.palette_layout(&tb) {
                if let Some((c, _)) = pl.swatches.iter().find(|(_, r)| r.contains(x, y)) {
                    self.set_color(*c);
                    self.palette_open = false;
                    self.refresh();
                    return;
                }
                if pl.fill.contains(x, y) {
                    self.style.fill = !self.style.fill;
                    self.refresh();
                    return;
                }
                if pl.panel.contains(x, y) {
                    return;
                }
            }
            if let Some((btn, _)) = tb.buttons.iter().find(|(_, r)| r.contains(x, y)).copied() {
                self.press(btn);
                return;
            }
            if tb.panel.contains(x, y) {
                return;
            }
        }
        if self.palette_open {
            self.palette_open = false;
            self.refresh();
        }
        let Some(sel) = self.sel else {
            self.drag = Drag::NewSel { start: (x, y), moved: false };
            self.refresh();
            return;
        };
        if self.mode == Mode::Text {
            self.drag = Drag::NewSel { start: (x, y), moved: false };
            return;
        }
        if let Some(h) = self.handle_at(x, y) {
            self.commit_text();
            self.drag = Drag::Resize { handle: h, orig: sel };
            self.selected_shape = None;
            self.refresh();
            return;
        }
        if self.editing.is_some() {
            self.commit_text();
            if self.tool != Tool::Text {
                self.refresh();
                return;
            }
        }
        let inside = sel.contains(x, y);
        if !inside && self.tool != Tool::Select {
            // Drawing tools draw only inside the region. Outside, they choose a new region, and
            // the tool is still in hand inside it. Select keeps moving and picking shapes.
            self.selected_shape = None;
            self.palette_open = false;
            self.reselect_from = self.sel.take();
            self.drag = Drag::NewSel { start: (x, y), moved: false };
            self.refresh();
            return;
        }
        match self.tool {
            Tool::Select => {
                let tol = 4.0 * self.scale();
                if let Some(i) = self.doc.hit(p, tol).filter(|_| inside) {
                    let s = (*self.doc.shapes()[i]).clone();
                    self.selected_shape = Some(i);
                    self.drag = Drag::MoveShape { idx: i, start: p, orig: s.clone(), cur: s, lifted: false };
                } else if inside {
                    self.selected_shape = None;
                    self.drag = Drag::MoveSel { start: (x, y), orig: sel };
                } else if self.doc.is_empty() {
                    // Start over with a new region.
                    self.selected_shape = None;
                    self.sel = None;
                    self.drag = Drag::NewSel { start: (x, y), moved: false };
                } else {
                    self.selected_shape = None;
                }
            }
            _ if !inside => {}
            Tool::Text => self.start_text(p),
            tool => {
                self.selected_shape = None;
                let st = self.style;
                let shape = match tool {
                    Tool::Rect => Shape::Rect { a: p, b: p, style: st },
                    Tool::Ellipse => Shape::Ellipse { a: p, b: p, style: st },
                    Tool::Line => Shape::Line { a: p, b: p, style: st },
                    Tool::Arrow => Shape::Arrow { a: p, b: p, style: st },
                    Tool::Draw => Shape::Pen { pts: vec![p], style: st },
                    Tool::Highlighter => Shape::Highlighter { pts: vec![p], style: st },
                    Tool::Counter => Shape::Counter { center: p, n: self.doc.next_counter(), style: st },
                    Tool::Blur => Shape::Blur { a: p, b: p, strength: (st.width * 2.5).max(6.0) },
                    Tool::Pixelate => Shape::Pixelate { a: p, b: p, block: (st.width * 3.0).max(8.0) },
                    Tool::Select | Tool::Text => unreachable!(),
                };
                self.baked.upto = 0;
                self.drag = Drag::Draw { shape, start: p };
                self.update_scratch();
            }
        }
        self.refresh();
    }

    fn on_mouse_move(&mut self, x: i32, y: i32) {
        if (x, y) == self.mouse && self.drag_is_none() {
            return;
        }
        self.mouse = (x, y);
        let p = Pt::new(x as f32, y as f32);
        let shift = key_down(VK_SHIFT.0);
        let bounds = self.bounds;
        match &mut self.drag {
            Drag::None => {
                self.update_hover();
                let hb = self.toolbar().and_then(|tb| tb.buttons.iter().find(|(_, r)| r.contains(x, y)).map(|(b, _)| *b));
                self.hover_btn = hb;
            }
            Drag::NewSel { start, moved } => {
                if !*moved && ((x - start.0).abs() > 3 || (y - start.1).abs() > 3) {
                    *moved = true;
                }
            }
            Drag::MoveSel { start, orig } => {
                self.sel = Some(orig.offset(x - start.0, y - start.1).clamp_inside(&bounds));
            }
            Drag::Resize { handle, orig } => {
                let (mut l, mut t, mut r, mut b) = (orig.x, orig.y, orig.right(), orig.bottom());
                match handle {
                    Handle::N => t = y,
                    Handle::S => b = y + 1,
                    Handle::E => r = x + 1,
                    Handle::W => l = x,
                    Handle::NE => (t, r) = (y, x + 1),
                    Handle::NW => (t, l) = (y, x),
                    Handle::SE => (b, r) = (y + 1, x + 1),
                    Handle::SW => (b, l) = (y + 1, x),
                }
                let n = IRect::from_ltrb(l.min(r - 1), t.min(b - 1), l.max(r), t.max(b)).intersect(&bounds);
                if !n.is_empty() {
                    self.sel = Some(n);
                }
            }
            Drag::Draw { shape, start } => {
                let start = *start;
                match shape {
                    Shape::Rect { b, .. } | Shape::Ellipse { b, .. } => *b = if shift { doc::snap_square(start, p) } else { p },
                    Shape::Line { b, .. } | Shape::Arrow { b, .. } => *b = if shift { doc::snap_angle(start, p) } else { p },
                    Shape::Blur { b, .. } | Shape::Pixelate { b, .. } => *b = p,
                    Shape::Pen { pts, .. } | Shape::Highlighter { pts, .. } => {
                        if shift {
                            // Shift draws a straight stroke from the start.
                            pts.truncate(1);
                            pts.push(doc::snap_angle(start, p));
                        } else if pts.last().is_none_or(|l| l.dist(p) >= 1.5) {
                            pts.push(p);
                        }
                    }
                    Shape::Counter { center, .. } => *center = p,
                    Shape::Text { .. } => {}
                }
                self.scratch_stale = true;
            }
            Drag::MoveShape { idx, start, orig, cur, lifted } => {
                let (dx, dy) = (p.x - start.x, p.y - start.y);
                let idx = *idx;
                let need_lift = !*lifted && (dx.abs() > 2.0 || dy.abs() > 2.0);
                if need_lift || *lifted {
                    let mut moved = orig.clone();
                    moved.translate(dx, dy);
                    *cur = moved;
                }
                if need_lift {
                    *lifted = true;
                    self.rebuild(Some(idx));
                }
                self.scratch_stale = true;
            }
        }
        self.refresh();
    }

    fn on_mouse_up(&mut self, x: i32, y: i32) {
        self.mouse = (x, y);
        unsafe {
            let _ = ReleaseCapture();
        }
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::NewSel { start, moved } => {
                let picked =
                    if moved { IRect::spanning(start.0, start.1, x, y).intersect(&self.bounds) } else { self.hover_window.unwrap_or_default() };
                if picked.w >= 2 && picked.h >= 2 {
                    self.sel = Some(picked);
                    self.reselect_from = None;
                    self.hover_window = None;
                    if self.mode == Mode::Text {
                        let img = self.base.crop(picked);
                        self.finish(Outcome::Ocr(img));
                        return;
                    }
                    if self.quick_finish_held() {
                        self.act_enter();
                        return;
                    }
                    if let Some(tool) = self.auto_tool.filter(|_| self.tool == Tool::Select && self.doc.is_empty()) {
                        self.set_tool(tool);
                    }
                } else if let Some(previous) = self.reselect_from.take() {
                    // A click outside the region, not a drag: keep the region.
                    self.sel = Some(previous);
                    self.update_hover();
                } else {
                    self.update_hover();
                }
            }
            Drag::Draw { shape, .. } => {
                self.commit_shape(shape);
                self.scratch_live = false;
            }
            Drag::MoveShape { idx, cur, lifted, .. } => {
                if lifted {
                    self.doc.replace(idx, cur);
                    self.rebuild(None);
                }
                self.scratch_live = false;
            }
            Drag::MoveSel { .. } | Drag::Resize { .. } | Drag::None => {}
        }
        self.refresh();
    }

    fn press(&mut self, btn: Btn) {
        match btn {
            Btn::Tool(t) => self.set_tool(t),
            Btn::Color => self.palette_open = !self.palette_open,
            Btn::Width => {
                self.style.width = doc::next_width(self.style.width, true);
                if self.style.width >= doc::WIDTHS[doc::WIDTHS.len() - 1] && self.hover_btn == Some(Btn::Width) {
                    // Clicking cycles: wrap to the smallest after the largest.
                    self.style.width = doc::WIDTHS[0];
                }
            }
            Btn::Undo => self.undo(),
            Btn::Redo => self.redo(),
            Btn::Ocr | Btn::Save | Btn::Copy => return self.act(btn),
            Btn::Close => return self.finish(Outcome::Cancel),
        }
        self.refresh();
    }

    fn set_tool(&mut self, t: Tool) {
        self.commit_text();
        self.tool = t;
        self.selected_shape = None;
        self.palette_open = false;
    }

    fn set_color(&mut self, c: Rgba) {
        self.style.color = c;
        if let Some(e) = &mut self.editing {
            e.style.color = c;
            self.update_scratch();
        }
    }

    fn undo(&mut self) {
        if self.editing.is_some() {
            // Undo while typing drops what was typed (or the change to an existing text).
            self.editing = None;
            self.rebuild(None);
            self.scratch_live = false;
            return;
        }
        if self.doc.undo() {
            self.selected_shape = None;
            self.rebuild(None);
        }
    }

    fn redo(&mut self) {
        if self.editing.is_none() && self.doc.redo() {
            self.selected_shape = None;
            self.rebuild(None);
        }
    }

    fn on_wheel(&mut self, delta: i16) {
        self.style.width = doc::next_width(self.style.width, delta > 0);
        if let Some(e) = &mut self.editing {
            e.style.width = self.style.width;
            self.update_scratch();
        }
        self.refresh();
    }

    fn on_key(&mut self, vk: u16) -> bool {
        let ctrl = key_down(VK_CONTROL.0);
        let shift = key_down(VK_SHIFT.0);
        if self.editing.is_some() {
            match vk {
                v if v == VK_ESCAPE.0 => {
                    self.commit_text();
                    self.refresh();
                }
                v if v == VK_RETURN.0 && ctrl => {
                    self.commit_text();
                    self.refresh();
                }
                v if v == VK_BACK.0 => {
                    if let Some(e) = &mut self.editing {
                        if ctrl {
                            let trimmed = e.text.trim_end_matches(|c: char| !c.is_whitespace()).len();
                            let keep = if trimmed == e.text.len() { e.text.trim_end().len() } else { trimmed };
                            e.text.truncate(keep);
                        } else {
                            e.text.pop();
                        }
                    }
                    self.after_text_change();
                }
                v if ctrl && v == b'Z' as u16 => {
                    self.undo();
                    self.refresh();
                }
                _ => return false,
            }
            return true;
        }
        match vk {
            v if v == VK_ESCAPE.0 => {
                if self.palette_open {
                    self.palette_open = false;
                } else if !self.drag_is_none() {
                    // Cancel the drag in progress.
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    self.drag = Drag::None;
                    self.scratch_live = false;
                    if let Some(previous) = self.reselect_from.take() {
                        self.sel = Some(previous);
                        self.update_hover();
                    }
                    self.rebuild(None);
                } else if self.finish_key.escape() && self.sel.is_some() {
                    // Esc finishes once a region is chosen. Before that there is nothing to keep.
                    self.confirm();
                    return true;
                } else {
                    self.finish(Outcome::Cancel);
                    return true;
                }
            }
            v if v == VK_RETURN.0 => {
                if self.finish_key.enter() {
                    self.confirm();
                }
                return true;
            }
            v if ctrl && v == b'C' as u16 => {
                self.act(Btn::Copy);
                return true;
            }
            v if ctrl && v == b'S' as u16 => {
                if shift {
                    if self.sel.is_none() {
                        self.sel = self.hover_window;
                    }
                    if let Some(img) = self.result_image() {
                        self.finish(Outcome::SaveAs(img));
                    }
                } else {
                    self.act(Btn::Save);
                }
                return true;
            }
            v if ctrl && (v == b'Z' as u16) => {
                if shift { self.redo() } else { self.undo() }
            }
            v if ctrl && v == b'Y' as u16 => self.redo(),
            v if ctrl && v == b'A' as u16 => {
                self.sel = Some(self.bounds);
                self.hover_window = None;
            }
            v if v == VK_DELETE.0 || v == VK_BACK.0 => {
                if let Some(i) = self.selected_shape.take() {
                    self.doc.remove(i);
                    self.rebuild(None);
                }
            }
            v if v == VK_LEFT.0 || v == VK_RIGHT.0 || v == VK_UP.0 || v == VK_DOWN.0 => {
                let step = if shift { 10 } else { 1 };
                let (dx, dy) = match v {
                    v if v == VK_LEFT.0 => (-step, 0),
                    v if v == VK_RIGHT.0 => (step, 0),
                    v if v == VK_UP.0 => (0, -step),
                    _ => (0, step),
                };
                match self.sel {
                    // Fine-tune the cursor while choosing.
                    None => unsafe {
                        let _ = SetCursorPos(self.mouse.0 + dx + self.origin.0, self.mouse.1 + dy + self.origin.1);
                    },
                    Some(sel) if ctrl => {
                        // Ctrl+arrows resize from the bottom-right corner.
                        let n = IRect::new(sel.x, sel.y, (sel.w + dx).max(2), (sel.h + dy).max(2)).intersect(&self.bounds);
                        self.sel = Some(n);
                    }
                    Some(sel) => self.sel = Some(sel.offset(dx, dy).clamp_inside(&self.bounds)),
                }
            }
            _ => return false,
        }
        self.refresh();
        true
    }

    fn on_char(&mut self, ch: char) {
        if let Some(e) = &mut self.editing {
            match ch {
                '\r' | '\n' => e.text.push('\n'),
                c if c.is_control() => return,
                c => e.text.push(c),
            }
            self.after_text_change();
            return;
        }
        if key_down(VK_CONTROL.0) {
            return;
        }
        match ch.to_ascii_uppercase() {
            'C' if self.sel.is_none() || matches!(self.drag, Drag::NewSel { .. }) => {
                if let Some((r, g, b)) = self.base.rgb_at(self.mouse.0, self.mouse.1) {
                    self.finish(Outcome::Color(Rgba::rgb(r, g, b)));
                }
            }
            'F' if self.sel.is_none() => {
                self.sel = Some(self.monitor_of(self.mouse.0, self.mouse.1));
                self.hover_window = None;
                if self.mode == Mode::Text {
                    let img = self.base.crop(self.sel.unwrap());
                    self.finish(Outcome::Ocr(img));
                    return;
                }
                self.refresh();
            }
            'O' if self.mode == Mode::Capture && self.sel.is_some() => self.act(Btn::Ocr),
            d @ '1'..='9' if self.mode == Mode::Capture => {
                let i = d as usize - '1' as usize;
                if let Some(c) = self.palette.get(i).copied() {
                    self.set_color(c);
                    self.refresh();
                }
            }
            c if self.mode == Mode::Capture && self.sel.is_some() => {
                if let Some(t) = self.tool_keys.tool(c) {
                    self.set_tool(t);
                    self.refresh();
                }
            }
            _ => {}
        }
    }

    fn cursor(&self) -> PCWSTR {
        let (x, y) = self.mouse;
        if let Some(tb) = self.toolbar() {
            if tb.panel.contains(x, y) || self.palette_layout(&tb).is_some_and(|p| p.panel.contains(x, y)) {
                return IDC_HAND;
            }
        }
        match &self.drag {
            Drag::MoveSel { .. } | Drag::MoveShape { .. } => return IDC_SIZEALL,
            Drag::Resize { handle, .. } => return handle_cursor(*handle),
            Drag::NewSel { .. } | Drag::Draw { .. } => return IDC_CROSS,
            Drag::None => {}
        }
        let Some(sel) = self.sel else { return IDC_CROSS };
        if self.mode == Mode::Text {
            return IDC_CROSS;
        }
        if let Some(h) = self.handle_at(x, y) {
            return handle_cursor(h);
        }
        if !sel.contains(x, y) {
            return if self.tool != Tool::Select || self.doc.is_empty() { IDC_CROSS } else { IDC_ARROW };
        }
        match self.tool {
            Tool::Select => {
                if self.doc.hit(Pt::new(x as f32, y as f32), 4.0 * self.scale()).is_some() { IDC_SIZEALL } else { IDC_SIZEALL }
            }
            Tool::Text => IDC_IBEAM,
            _ => IDC_CROSS,
        }
    }
}

fn handle_cursor(h: Handle) -> PCWSTR {
    match h {
        Handle::N | Handle::S => IDC_SIZENS,
        Handle::E | Handle::W => IDC_SIZEWE,
        Handle::NW | Handle::SE => IDC_SIZENWSE,
        Handle::NE | Handle::SW => IDC_SIZENESW,
    }
}

const MAG_CELLS: u32 = 15;
const MAG_ZOOM: f32 = 8.0;

fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Overlay;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            OPEN.with(|o| *o.borrow_mut() = None);
            drop(Box::from_raw(ptr));
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let ov = &mut *ptr;
        match msg {
            WM_PAINT => {
                ov.paint();
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEMOVE => {
                let (x, y) = lparam_xy(lp);
                ov.on_mouse_move(x, y);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = lparam_xy(lp);
                ov.on_mouse_down(x, y);
                LRESULT(0)
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = lparam_xy(lp);
                let on_toolbar = ov.toolbar().is_some_and(|tb| tb.panel.contains(x, y));
                if ov.mode == Mode::Capture
                    && !on_toolbar
                    && ov.sel.is_some_and(|s| s.contains(x, y))
                    && matches!(ov.tool, Tool::Select)
                    && ov.doc.hit(Pt::new(x as f32, y as f32), 4.0).is_none()
                {
                    ov.act_enter();
                } else {
                    ov.on_mouse_down(x, y);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = lparam_xy(lp);
                ov.on_mouse_up(x, y);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                if ov.editing.is_some() {
                    ov.commit_text();
                } else if ov.sel.is_none() || ov.mode == Mode::Text {
                    ov.finish(Outcome::Cancel);
                    return LRESULT(0);
                } else if ov.doc.is_empty() && ov.drag_is_none() {
                    // Right-click before drawing anything: choose again.
                    ov.sel = None;
                    ov.palette_open = false;
                    ov.update_hover();
                }
                ov.refresh();
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                if ov.sel.is_some() && ov.mode == Mode::Capture {
                    ov.on_wheel(((wp.0 >> 16) & 0xFFFF) as i16);
                }
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if ov.on_key(wp.0 as u16) {
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
            WM_CHAR => {
                if let Some(c) = char::from_u32(wp.0 as u32) {
                    ov.on_char(c);
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                if (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                    let id = ov.cursor();
                    SetCursor(LoadCursorW(None, id).ok());
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_TIMER if wp.0 == CARET_TIMER => {
                ov.caret_on = !ov.caret_on;
                ov.refresh();
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                // Lost the mouse mid-drag (e.g. a system dialog): drop the drag cleanly.
                if !ov.drag_is_none() && HWND(lp.0 as *mut _) != hwnd {
                    let (x, y) = ov.mouse;
                    ov.on_mouse_up(x, y);
                }
                LRESULT(0)
            }
            // The overlay spans monitors with different DPIs; never let Windows resize it.
            WM_DPICHANGED => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Render the overlay offscreen in a scripted state and return the frame. Used by
/// `--selftest-render` to check the look without taking over the screen.
pub fn render_offscreen(script: &str) -> Option<Image> {
    let vs = capture::virtual_screen();
    let base = capture::grab(vs, false)?;
    let (w, h) = (base.w, base.h);
    let back = make_back(w as i32, h as i32)?;
    let origin = (vs.x, vs.y);
    let mut ov = Overlay {
        hwnd: HWND::default(),
        main: SendHwnd(0),
        mode: Mode::Capture,
        origin,
        bounds: IRect::new(0, 0, w as i32, h as i32),
        dim: base.dimmed(DIM_KEEP),
        committed: base.clone(),
        base,
        scratch: Image::new(0, 0),
        scratch_live: false,
        scratch_stale: false,
        baked: Baked::default(),
        back,
        windows: capture::windows_front_to_back(HWND::default(), origin),
        monitors: capture::monitors(origin),
        sel: None,
        doc: Doc::default(),
        tool: Tool::Select,
        style: Style::default(),
        palette: Rgba::DEFAULT_PALETTE.to_vec(),
        palette_open: false,
        drag: Drag::None,
        mouse: (w as i32 / 3, h as i32 / 3),
        hover_window: None,
        hover_btn: None,
        selected_shape: None,
        editing: None,
        caret_on: true,
        magnifier: true,
        enter: Default::default(),
        finish_key: Default::default(),
        quick_finish: prism_core::settings::QuickFinish::Off,
        auto_tool: None,
        reselect_from: None,
        tool_keys: Default::default(),
        text: GdiText::new(),
        last_bounds: IRect::default(),
    };
    let (cx, cy) = (w as i32 / 2, h as i32 / 2);
    match script {
        "hover" => ov.update_hover(),
        "selecting" => {
            ov.drag = Drag::NewSel { start: (cx - 400, cy - 250), moved: true };
            ov.mouse = (cx + 300, cy + 200);
        }
        "text-mode" => {
            ov.mode = Mode::Text;
            ov.update_hover();
        }
        _ => {
            // "annotated": a selection with one of each shape, the palette open and a tooltip.
            let sel = IRect::new(cx - 450, cy - 280, 900, 520);
            ov.sel = Some(sel);
            let st = |c: Rgba, w: f32| Style { color: c, width: w, fill: false };
            let (x, y) = (sel.x as f32, sel.y as f32);
            let shapes = vec![
                Shape::Rect { a: Pt::new(x + 40.0, y + 40.0), b: Pt::new(x + 260.0, y + 160.0), style: st(Rgba::RED, 4.0) },
                Shape::Ellipse { a: Pt::new(x + 300.0, y + 40.0), b: Pt::new(x + 460.0, y + 160.0), style: st(Rgba::GREEN, 4.0) },
                Shape::Arrow { a: Pt::new(x + 520.0, y + 200.0), b: Pt::new(x + 700.0, y + 60.0), style: st(Rgba::ORANGE, 4.0) },
                Shape::Highlighter {
                    pts: vec![Pt::new(x + 40.0, y + 220.0), Pt::new(x + 400.0, y + 222.0)],
                    style: st(Rgba::YELLOW, 4.0),
                },
                Shape::Pen {
                    pts: (0..40).map(|i| Pt::new(x + 40.0 + i as f32 * 9.0, y + 290.0 + (i as f32 * 0.5).sin() * 18.0)).collect(),
                    style: st(Rgba::BLUE, 3.0),
                },
                Shape::Counter { center: Pt::new(x + 760.0, y + 260.0), n: 1, style: st(Rgba::RED, 4.0) },
                Shape::Counter { center: Pt::new(x + 820.0, y + 260.0), n: 2, style: st(Rgba::RED, 4.0) },
                Shape::Blur { a: Pt::new(x + 40.0, y + 350.0), b: Pt::new(x + 300.0, y + 480.0), strength: 10.0 },
                Shape::Pixelate { a: Pt::new(x + 330.0, y + 350.0), b: Pt::new(x + 560.0, y + 480.0), block: 12.0 },
                Shape::Line { a: Pt::new(x + 600.0, y + 360.0), b: Pt::new(x + 860.0, y + 360.0), style: st(Rgba::PURPLE, 6.0) },
            ];
            for s in shapes {
                ov.commit_shape(s);
            }
            let (tw, th) = ov.text.measure("Hello, annotations", doc::text_px(4.0), false);
            ov.commit_shape(Shape::Text {
                pos: Pt::new(x + 600.0, y + 400.0),
                text: "Hello, annotations".into(),
                style: st(Rgba::WHITE, 4.0),
                w: tw as f32,
                h: th as f32,
            });
            ov.tool = Tool::Arrow;
            ov.palette_open = script == "palette";
            ov.selected_shape = if script == "palette" { None } else { Some(1) };
            if let Some(tb) = ov.toolbar() {
                ov.hover_btn = Some(Btn::Tool(Tool::Highlighter));
                ov.mouse = tb.buttons.iter().find(|(b, _)| *b == Btn::Tool(Tool::Highlighter)).map(|(_, r)| (r.x + 5, r.y + 5)).unwrap();
            }
        }
    }
    ov.compose(ov.bounds);
    let len = w as usize * h as usize * 4;
    unsafe {
        let _ = GdiFlush();
    }
    let data = unsafe { std::slice::from_raw_parts(ov.back.bits, len) }.to_vec();
    Image::from_bgra(w, h, data)
}
