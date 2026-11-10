//! Drawing the overlay's chrome: panels, text, toolbar icons. Everything is vector-drawn at the
//! monitor's scale, so it is sharp at any DPI and needs no image assets.
//!
//! Colours are given as straight RGBA and converted for the BGRA canvas by `paint_color`.

use crate::text::GdiText;
use prism_core::doc::Tool;
use prism_core::settings::ToolbarPlace;
use prism_core::icon::rounded_rect;
use prism_core::render::{blend_mask, paint_color};
use prism_core::{IRect, Rgba};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, PixmapMut, Rect, Stroke, StrokeDash, Transform};

pub const PANEL: Rgba = Rgba { r: 0x1C, g: 0x1C, b: 0x21, a: 0xF2 };
pub const PANEL_BORDER: Rgba = Rgba { r: 0xFF, g: 0xFF, b: 0xFF, a: 0x1A };
pub const HOVER: Rgba = Rgba { r: 0xFF, g: 0xFF, b: 0xFF, a: 0x16 };
pub const ACTIVE: Rgba = Rgba { r: 0x4C, g: 0x8D, b: 0xFF, a: 0x40 };
pub const ACCENT: Rgba = Rgba::rgb(0x4C, 0x8D, 0xFF);
pub const FG: Rgba = Rgba::rgb(0xEE, 0xEE, 0xF2);
pub const FG_DIM: Rgba = Rgba::rgb(0xA1, 0xA1, 0xAA);
pub const FG_DISABLED: Rgba = Rgba { r: 0xEE, g: 0xEE, b: 0xF2, a: 0x55 };

pub fn solid(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(paint_color(c));
    p.anti_alias = true;
    p
}

pub fn fill_rrect(pm: &mut PixmapMut, r: IRect, radius: f32, c: Rgba) {
    if let Some(path) = rounded_rect(r.x as f32, r.y as f32, r.w as f32, r.h as f32, radius) {
        pm.fill_path(&path, &solid(c), FillRule::Winding, Transform::identity(), None);
    }
}

pub fn stroke_rrect(pm: &mut PixmapMut, r: IRect, radius: f32, width: f32, c: Rgba) {
    let h = width / 2.0;
    if let Some(path) = rounded_rect(r.x as f32 + h, r.y as f32 + h, r.w as f32 - width, r.h as f32 - width, radius) {
        pm.stroke_path(&path, &solid(c), &Stroke { width, ..Stroke::default() }, Transform::identity(), None);
    }
}

pub fn fill_rect(pm: &mut PixmapMut, r: IRect, c: Rgba) {
    if let Some(rect) = Rect::from_xywh(r.x as f32, r.y as f32, r.w as f32, r.h as f32) {
        pm.fill_rect(rect, &solid(c), Transform::identity(), None);
    }
}

/// A floating panel: soft shadow, fill, hairline border.
pub fn panel(pm: &mut PixmapMut, r: IRect, radius: f32, scale: f32) {
    for i in 1..=4 {
        let spread = (i as f32 * 2.0 * scale) as i32;
        let alpha = (22 - i * 4) as u8;
        fill_rrect(pm, r.inflate(spread).offset(0, (scale * 2.0) as i32), radius + spread as f32, Rgba { r: 0, g: 0, b: 0, a: alpha });
    }
    fill_rrect(pm, r, radius, PANEL);
    stroke_rrect(pm, r, radius, 1.0, PANEL_BORDER);
}

/// `r` grown by `d` on every side, but kept on the canvas: where the selection meets the edge of
/// the screen, its frame is drawn just inside instead of off-screen.
fn grow_on_canvas(pm: &PixmapMut, r: IRect, d: i32) -> IRect {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    IRect::from_ltrb((r.x - d).max(0), (r.y - d).max(0), (r.right() + d).min(w), (r.bottom() + d).min(h))
}

/// How the selection's frame is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// The window or screen a click would pick: a plain accent outline.
    Hover,
    /// A region being dragged out, moved or resized: the frame plus guides at the thirds.
    Dragging,
    /// A chosen region while drawing in it: just the frame.
    Plain,
    /// A chosen region: the frame plus grips on the edges to resize it by.
    Chosen,
}

/// The selection's frame: a thin white line with a soft shadow, and bracket corners.
pub fn selection_frame(pm: &mut PixmapMut, r: IRect, kind: Frame, scale: f32) {
    let line = scale.max(1.0).round() as i32;
    if kind == Frame::Hover {
        let w = (2.0 * scale).max(2.0).round() as i32;
        let f = grow_on_canvas(pm, r, w);
        stroke_rect_px(pm, grow_on_canvas(pm, f, 1), 1.0, Rgba { r: 0, g: 0, b: 0, a: 0x40 });
        stroke_rect_px(pm, f, w as f32, ACCENT);
        return;
    }
    if kind == Frame::Dragging && r.w as f32 > 90.0 * scale && r.h as f32 > 90.0 * scale {
        let guide = Rgba { r: 0xFF, g: 0xFF, b: 0xFF, a: 0x30 };
        for i in 1..3 {
            fill_rect(pm, IRect::new(r.x + r.w * i / 3, r.y, 1, r.h), guide);
            fill_rect(pm, IRect::new(r.x, r.y + r.h * i / 3, r.w, 1), guide);
        }
    }
    // A soft shadow outside, so the line reads on light screens as well as dark ones.
    let f = grow_on_canvas(pm, r, line);
    for (i, a) in [0x48u8, 0x24, 0x10].iter().enumerate() {
        stroke_rect_px(pm, grow_on_canvas(pm, f, i as i32 + 1), 1.0, Rgba { r: 0, g: 0, b: 0, a: *a });
    }
    stroke_rect_px(pm, f, line as f32, Rgba { r: 0xFF, g: 0xFF, b: 0xFF, a: 0xE6 });
    // Bracket corners, and grips at the middle of each edge once it can be resized.
    let t = (3.0 * scale).max(2.0).round() as i32;
    let o = grow_on_canvas(pm, r, t);
    let len = ((18.0 * scale) as i32).min(r.w.max(r.h) / 2).max(t * 2);
    let (lx, ly) = (len.min(o.w / 2).max(t), len.min(o.h / 2).max(t));
    let mut bars = vec![
        IRect::new(o.x, o.y, lx, t),
        IRect::new(o.x, o.y, t, ly),
        IRect::new(o.right() - lx, o.y, lx, t),
        IRect::new(o.right() - t, o.y, t, ly),
        IRect::new(o.x, o.bottom() - t, lx, t),
        IRect::new(o.x, o.bottom() - ly, t, ly),
        IRect::new(o.right() - lx, o.bottom() - t, lx, t),
        IRect::new(o.right() - t, o.bottom() - ly, t, ly),
    ];
    if kind == Frame::Chosen {
        let g = (22.0 * scale) as i32;
        if o.w > g + 2 * lx + 2 * t {
            bars.push(IRect::new(o.x + (o.w - g) / 2, o.y, g, t));
            bars.push(IRect::new(o.x + (o.w - g) / 2, o.bottom() - t, g, t));
        }
        if o.h > g + 2 * ly + 2 * t {
            bars.push(IRect::new(o.x, o.y + (o.h - g) / 2, t, g));
            bars.push(IRect::new(o.right() - t, o.y + (o.h - g) / 2, t, g));
        }
    }
    let r_ = t as f32 / 2.0;
    for b in &bars {
        fill_rrect(pm, b.inflate(1), r_ + 1.0, Rgba { r: 0, g: 0, b: 0, a: 0x50 });
    }
    for b in &bars {
        fill_rrect(pm, *b, r_, Rgba::WHITE);
    }
}

fn stroke_rect_px(pm: &mut PixmapMut, r: IRect, width: f32, c: Rgba) {
    let w = width as i32;
    fill_rect(pm, IRect::new(r.x, r.y, r.w, w), c);
    fill_rect(pm, IRect::new(r.x, r.bottom() - w, r.w, w), c);
    fill_rect(pm, IRect::new(r.x, r.y + w, w, r.h - 2 * w), c);
    fill_rect(pm, IRect::new(r.right() - w, r.y + w, w, r.h - 2 * w), c);
}

pub fn dashed_rect(pm: &mut PixmapMut, r: IRect, c: Rgba, scale: f32) {
    let Some(rect) = Rect::from_xywh(r.x as f32 + 0.5, r.y as f32 + 0.5, r.w as f32 - 1.0, r.h as f32 - 1.0) else { return };
    let path = PathBuilder::from_rect(rect);
    let s = Stroke { width: scale.max(1.0), dash: StrokeDash::new(vec![4.0 * scale, 3.0 * scale], 0.0), ..Stroke::default() };
    pm.stroke_path(&path, &solid(Rgba { r: 0, g: 0, b: 0, a: 0x90 }), &Stroke { width: scale.max(1.0) + 1.0, ..Stroke::default() }, Transform::identity(), None);
    pm.stroke_path(&path, &solid(c), &s, Transform::identity(), None);
}

/// Draw text with its top-left at (x, y).
pub fn text(pm: &mut PixmapMut, t: &GdiText, s: &str, px: f32, bold: bool, x: i32, y: i32, c: Rgba) {
    if let Some(m) = t.mask(s, px, bold) {
        blend_mask(pm, x, y, &m, c);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Btn {
    Tool(Tool),
    Color,
    Width,
    Undo,
    Redo,
    Ocr,
    Save,
    Copy,
    Close,
}

impl Btn {
    pub fn tooltip(self, keys: &prism_core::doc::ToolKeys) -> String {
        match self {
            Btn::Tool(Tool::Select) => format!("{}  {}   Shift+drag for a new region", Tool::Select.label(), keys.key(Tool::Select)),
            Btn::Tool(Tool::Highlighter) => format!(
                "{}  {}   Start on a word to snap to it, hold Shift or Ctrl not to",
                Tool::Highlighter.label(),
                keys.key(Tool::Highlighter)
            ),
            Btn::Tool(t) => format!("{}  {}", t.label(), keys.key(t)),
            Btn::Color => "Colour  1–8".into(),
            Btn::Width => "Size  scroll wheel".into(),
            Btn::Undo => "Undo  Ctrl+Z".into(),
            Btn::Redo => "Redo  Ctrl+Y".into(),
            Btn::Ocr => "Copy text  O".into(),
            Btn::Save => "Save  Ctrl+S".into(),
            Btn::Copy => "Copy  Ctrl+C".into(),
            Btn::Close => "Close  Esc".into(),
        }
    }
}

/// The toolbar's contents. `None` is a separator.
pub fn toolbar_items() -> Vec<Option<Btn>> {
    let mut v: Vec<Option<Btn>> = Tool::ALL.iter().map(|t| Some(Btn::Tool(*t))).collect();
    v.extend([None, Some(Btn::Color), Some(Btn::Width), None, Some(Btn::Undo), Some(Btn::Redo), None, Some(Btn::Ocr), Some(Btn::Save), Some(Btn::Copy), Some(Btn::Close)]);
    v
}

/// The side of the toolbar that tooltips and the palette open on: away from the region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Above,
    Below,
    Left,
    Right,
}

pub struct ToolbarLayout {
    pub panel: IRect,
    pub buttons: Vec<(Btn, IRect)>,
    pub separators: Vec<IRect>,
    pub opens: Side,
}

pub fn layout_toolbar(sel: IRect, monitor: IRect, scale: f32, place: ToolbarPlace) -> ToolbarLayout {
    let b = (32.0 * scale).round() as i32;
    let pad = (4.0 * scale).round() as i32;
    let sep = (9.0 * scale).round() as i32;
    let gap = (8.0 * scale).round() as i32;
    let items = toolbar_items();
    let long = pad * 2 + items.iter().map(|i| if i.is_some() { b } else { sep }).sum::<i32>();
    let short = b + pad * 2;
    // A side that's too short for the toolbar standing up gets it lying below instead.
    let vertical = matches!(place, ToolbarPlace::Left | ToolbarPlace::Right) && long <= monitor.h - 2 * gap;
    let place = if !vertical && matches!(place, ToolbarPlace::Left | ToolbarPlace::Right) { ToolbarPlace::BelowRight } else { place };
    let (panel, opens) = if vertical {
        let (w, h) = (short, long);
        let y = sel.y.min(monitor.bottom() - gap - h).max(monitor.y + gap);
        let right_fits = sel.right() + gap + w <= monitor.right() - gap;
        let left_fits = sel.x - gap - w >= monitor.x + gap;
        let (x, opens) = match place {
            ToolbarPlace::Left if left_fits => (sel.x - gap - w, Side::Left),
            ToolbarPlace::Right if right_fits => (sel.right() + gap, Side::Right),
            _ if right_fits => (sel.right() + gap, Side::Right),
            _ if left_fits => (sel.x - gap - w, Side::Left),
            // No room either side: inside the edge it was asked for.
            ToolbarPlace::Left => (sel.x + gap, Side::Right),
            _ => (sel.right() - gap - w, Side::Left),
        };
        (IRect::new(x, y, w, h), opens)
    } else {
        let (w, h) = (long, short);
        let x = match place {
            ToolbarPlace::BelowLeft | ToolbarPlace::AboveLeft => sel.x,
            ToolbarPlace::BelowCenter | ToolbarPlace::AboveCenter => sel.x + (sel.w - w) / 2,
            _ => sel.right() - w,
        };
        let x = x.min(monitor.right() - w - gap).max(monitor.x + gap);
        let below_fits = sel.bottom() + gap + h <= monitor.bottom() - gap;
        let above_fits = sel.y - gap - h >= monitor.y + gap;
        let wants_above = matches!(place, ToolbarPlace::AboveLeft | ToolbarPlace::AboveCenter | ToolbarPlace::AboveRight);
        // The side asked for, then the other side, then inside the edge asked for.
        let (y, opens) = match (wants_above, above_fits, below_fits) {
            (true, true, _) | (false, true, false) => (sel.y - gap - h, Side::Above),
            (false, _, true) | (true, false, true) => (sel.bottom() + gap, Side::Below),
            (true, false, false) => (sel.y + gap, Side::Below),
            (false, false, false) => (sel.bottom() - gap - h, Side::Above),
        };
        (IRect::new(x, y, w, h), opens)
    };
    let mut buttons = Vec::new();
    let mut separators = Vec::new();
    let mut at = if vertical { panel.y + pad } else { panel.x + pad };
    for item in items {
        let step = if item.is_some() { b } else { sep };
        match (item, vertical) {
            (Some(btn), false) => buttons.push((btn, IRect::new(at, panel.y + pad, b, b))),
            (Some(btn), true) => buttons.push((btn, IRect::new(panel.x + pad, at, b, b))),
            (None, false) => separators.push(IRect::new(at + sep / 2, panel.y + pad + b / 4, 1, b / 2)),
            (None, true) => separators.push(IRect::new(panel.x + pad + b / 4, at + sep / 2, b / 2, 1)),
        }
        at += step;
    }
    ToolbarLayout { panel, buttons, separators, opens }
}

/// Where a popup of size `w`×`h` beside `anchor` goes, on the `opens` side, kept on the monitor.
pub fn beside(anchor: IRect, opens: Side, w: i32, h: i32, gap: i32, monitor: IRect) -> IRect {
    let cx = (anchor.x + anchor.w / 2 - w / 2).clamp(monitor.x + 4, (monitor.right() - w - 4).max(monitor.x + 4));
    let cy = (anchor.y + anchor.h / 2 - h / 2).clamp(monitor.y + 4, (monitor.bottom() - h - 4).max(monitor.y + 4));
    match opens {
        Side::Above => IRect::new(cx, anchor.y - gap - h, w, h),
        Side::Below => IRect::new(cx, anchor.bottom() + gap, w, h),
        Side::Left => IRect::new(anchor.x - gap - w, cy, w, h),
        Side::Right => IRect::new(anchor.right() + gap, cy, w, h),
    }
}

/// Palette popup: one swatch per colour, then a fill toggle.
pub struct PaletteLayout {
    pub panel: IRect,
    pub swatches: Vec<(Rgba, IRect)>,
    pub fill: IRect,
}

pub fn layout_palette(anchor: IRect, opens: Side, colors: &[Rgba], monitor: IRect, scale: f32) -> PaletteLayout {
    let s = (26.0 * scale).round() as i32;
    let pad = (6.0 * scale).round() as i32;
    let gap = (4.0 * scale).round() as i32;
    let n = colors.len() as i32 + 1;
    let width = pad * 2 + n * s + (n - 1) * gap + gap * 2;
    let height = s + pad * 2;
    let panel = beside(anchor, opens, width, height, (10.0 * scale) as i32, monitor);
    let (x, y) = (panel.x, panel.y);
    let mut swatches = Vec::new();
    let mut cx = x + pad;
    for c in colors {
        swatches.push((*c, IRect::new(cx, y + pad, s, s)));
        cx += s + gap;
    }
    let fill = IRect::new(cx + gap * 2, y + pad, s, s);
    PaletteLayout { panel, swatches, fill }
}

fn stroke_of(w: f32) -> Stroke {
    Stroke { width: w, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() }
}

/// Draw a toolbar icon centred in `r`.
pub fn icon(pm: &mut PixmapMut, t: &GdiText, btn: Btn, r: IRect, c: Rgba, scale: f32, extra: IconExtra) {
    let s = 18.0 * scale;
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let (l, tp) = (cx - s / 2.0, cy - s / 2.0);
    // Points in an 18×18 design grid.
    let p = |x: f32, y: f32| (l + x * scale, tp + y * scale);
    let paint = solid(c);
    let sw = 1.6 * scale;
    let id = Transform::identity();
    let mut pb = PathBuilder::new();
    let mut fill_paths: Vec<tiny_skia::Path> = Vec::new();
    match btn {
        Btn::Tool(Tool::Select) => {
            let pts = [(4.0, 2.5), (4.0, 15.0), (7.4, 11.8), (9.8, 16.4), (11.8, 15.4), (9.5, 10.9), (14.0, 10.6)];
            let (x0, y0) = p(pts[0].0, pts[0].1);
            pb.move_to(x0, y0);
            for (x, y) in &pts[1..] {
                let (x, y) = p(*x, *y);
                pb.line_to(x, y);
            }
            pb.close();
        }
        Btn::Tool(Tool::Rect) => {
            let (x, y) = p(3.0, 4.0);
            if let Some(path) = rounded_rect(x, y, 12.0 * scale, 10.0 * scale, 1.5 * scale) {
                pm.stroke_path(&path, &paint, &stroke_of(sw), id, None);
            }
        }
        Btn::Tool(Tool::Ellipse) => {
            let (x, y) = p(9.0, 9.0);
            if let Some(path) = PathBuilder::from_circle(x, y, 6.2 * scale) {
                pm.stroke_path(&path, &paint, &stroke_of(sw), id, None);
            }
        }
        Btn::Tool(Tool::Line) => {
            let (a, b) = (p(3.5, 14.5), p(14.5, 3.5));
            pb.move_to(a.0, a.1);
            pb.line_to(b.0, b.1);
        }
        Btn::Tool(Tool::Arrow) => {
            let (a, b) = (p(3.5, 14.5), p(14.0, 4.0));
            pb.move_to(a.0, a.1);
            pb.line_to(b.0, b.1);
            let (h1, h2) = (p(7.5, 4.0), p(14.0, 10.5));
            pb.move_to(h1.0, h1.1);
            pb.line_to(b.0, b.1);
            pb.line_to(h2.0, h2.1);
        }
        Btn::Tool(Tool::Draw) => {
            let a = p(2.5, 12.5);
            pb.move_to(a.0, a.1);
            let (c1, m) = (p(5.0, 4.0), p(8.5, 9.0));
            pb.quad_to(c1.0, c1.1, m.0, m.1);
            let (c2, e) = (p(12.0, 14.5), p(15.5, 5.5));
            pb.quad_to(c2.0, c2.1, e.0, e.1);
        }
        Btn::Tool(Tool::Highlighter) => {
            let (a, b) = (p(4.0, 13.0), p(14.0, 5.0));
            let mut hb = PathBuilder::new();
            hb.move_to(a.0, a.1);
            hb.line_to(b.0, b.1);
            if let Some(path) = hb.finish() {
                let mut tint = c;
                tint.a = (c.a as u16 * 110 / 255) as u8;
                pm.stroke_path(&path, &solid(tint), &Stroke { width: 6.0 * scale, line_cap: LineCap::Square, ..Stroke::default() }, id, None);
            }
            let (u1, u2) = (p(2.5, 16.0), p(15.5, 16.0));
            pb.move_to(u1.0, u1.1);
            pb.line_to(u2.0, u2.1);
        }
        Btn::Tool(Tool::Text) => {
            let (a, b) = (p(4.0, 4.0), p(14.0, 4.0));
            pb.move_to(a.0, a.1 + 1.5 * scale);
            pb.line_to(a.0, a.1);
            pb.line_to(b.0, b.1);
            pb.line_to(b.0, b.1 + 1.5 * scale);
            let (m1, m2) = (p(9.0, 4.0), p(9.0, 15.0));
            pb.move_to(m1.0, m1.1);
            pb.line_to(m2.0, m2.1);
            let (f1, f2) = (p(7.0, 15.0), p(11.0, 15.0));
            pb.move_to(f1.0, f1.1);
            pb.line_to(f2.0, f2.1);
        }
        Btn::Tool(Tool::Counter) => {
            let (x, y) = p(9.0, 9.0);
            if let Some(path) = PathBuilder::from_circle(x, y, 7.0 * scale) {
                pm.stroke_path(&path, &paint, &stroke_of(sw), id, None);
            }
            if let Some(m) = t.mask("1", 11.0 * scale, true) {
                blend_mask(pm, (x - m.w as f32 / 2.0).round() as i32, (y - m.h as f32 / 2.0).round() as i32, &m, c);
            }
        }
        Btn::Tool(Tool::Blur) => {
            // A droplet.
            let top = p(9.0, 2.5);
            let mut db = PathBuilder::new();
            db.move_to(top.0, top.1);
            let (c1, a1) = (p(15.0, 9.0), p(14.0, 11.5));
            db.quad_to(c1.0, c1.1, a1.0, a1.1);
            let (c2, a2) = (p(12.5, 15.8), p(9.0, 15.8));
            db.quad_to(c2.0, c2.1, a2.0, a2.1);
            let (c3, a3) = (p(5.5, 15.8), p(4.0, 11.5));
            db.quad_to(c3.0, c3.1, a3.0, a3.1);
            let (c4, a4) = (p(3.0, 9.0), top);
            db.quad_to(c4.0, c4.1, a4.0, a4.1);
            db.close();
            if let Some(path) = db.finish() {
                pm.stroke_path(&path, &paint, &stroke_of(sw), id, None);
            }
        }
        Btn::Tool(Tool::Pixelate) => {
            let cell = 4.0 * scale;
            for (i, (gx, gy)) in [(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1), (0, 2), (1, 2), (2, 2)].iter().enumerate() {
                let (x, y) = p(3.0, 3.0);
                let alpha = [255u8, 120, 200, 140, 255, 90, 210, 100, 255][i];
                if let Some(rect) = Rect::from_xywh(x + *gx as f32 * cell, y + *gy as f32 * cell, cell - 0.6 * scale, cell - 0.6 * scale) {
                    pm.fill_rect(rect, &solid(Rgba { a: (c.a as u16 * alpha as u16 / 255) as u8, ..c }), id, None);
                }
            }
        }
        Btn::Color => {
            let (x, y) = p(9.0, 9.0);
            if let Some(path) = PathBuilder::from_circle(x, y, 7.0 * scale) {
                pm.fill_path(&path, &solid(extra.color), FillRule::Winding, id, None);
                pm.stroke_path(&path, &solid(Rgba { r: 255, g: 255, b: 255, a: 0x70 }), &stroke_of(1.2 * scale), id, None);
            }
        }
        Btn::Width => {
            let (x, y) = p(9.0, 9.0);
            let rad = (extra.width * 0.5 * scale).clamp(1.5 * scale, 7.5 * scale);
            if let Some(path) = PathBuilder::from_circle(x, y, rad) {
                pm.fill_path(&path, &paint, FillRule::Winding, id, None);
            }
        }
        Btn::Undo | Btn::Redo => {
            let flip = btn == Btn::Redo;
            let q = |x: f32, y: f32| if flip { p(18.0 - x, y) } else { p(x, y) };
            let a = q(4.5, 7.0);
            pb.move_to(a.0, a.1);
            let b = q(11.0, 7.0);
            pb.line_to(b.0, b.1);
            let (c1, e) = (q(15.5, 7.0), q(15.5, 11.0));
            pb.quad_to(c1.0, c1.1, e.0, e.1);
            let (c2, f) = (q(15.5, 15.0), q(11.0, 15.0));
            pb.quad_to(c2.0, c2.1, f.0, f.1);
            let g = q(7.0, 15.0);
            pb.line_to(g.0, g.1);
            let (h1, h2) = (q(7.5, 4.0), q(7.5, 10.0));
            pb.move_to(h1.0, h1.1);
            pb.line_to(a.0, a.1);
            pb.line_to(h2.0, h2.1);
        }
        Btn::Ocr => {
            for (x, y, dx, dy) in [(2.5, 2.5, 1.0, 1.0), (15.5, 2.5, -1.0, 1.0), (2.5, 15.5, 1.0, -1.0), (15.5, 15.5, -1.0, -1.0)] {
                let (sx, sy) = p(x, y);
                pb.move_to(sx, sy + dy * 3.5 * scale);
                pb.line_to(sx, sy);
                pb.line_to(sx + dx * 3.5 * scale, sy);
            }
            for (y, w) in [(6.5, 8.0), (9.0, 8.0), (11.5, 5.0)] {
                let (a, b) = (p(5.0, y), p(5.0 + w, y));
                pb.move_to(a.0, a.1);
                pb.line_to(b.0, b.1);
            }
        }
        Btn::Save => {
            let (a, b) = (p(9.0, 2.5), p(9.0, 11.0));
            pb.move_to(a.0, a.1);
            pb.line_to(b.0, b.1);
            let (h1, h2) = (p(5.5, 7.5), p(12.5, 7.5));
            pb.move_to(h1.0, h1.1);
            pb.line_to(b.0, b.1);
            pb.line_to(h2.0, h2.1);
            let (t1, t2, t3, t4) = (p(3.0, 11.5), p(3.0, 15.0), p(15.0, 15.0), p(15.0, 11.5));
            pb.move_to(t1.0, t1.1);
            pb.line_to(t2.0, t2.1);
            pb.line_to(t3.0, t3.1);
            pb.line_to(t4.0, t4.1);
        }
        Btn::Copy => {
            let (x, y) = p(6.0, 6.0);
            if let Some(path) = rounded_rect(x, y, 9.5 * scale, 9.5 * scale, 2.0 * scale) {
                pm.stroke_path(&path, &paint, &stroke_of(sw), id, None);
            }
            let (a, b, c2, d) = (p(3.0, 11.5), p(3.0, 4.5), p(4.5, 3.0), p(11.5, 3.0));
            pb.move_to(a.0, a.1);
            pb.line_to(b.0, b.1);
            pb.quad_to(p(3.0, 3.0).0, p(3.0, 3.0).1, c2.0, c2.1);
            pb.line_to(d.0, d.1);
        }
        Btn::Close => {
            let (a, b, c2, d) = (p(4.5, 4.5), p(13.5, 13.5), p(13.5, 4.5), p(4.5, 13.5));
            pb.move_to(a.0, a.1);
            pb.line_to(b.0, b.1);
            pb.move_to(c2.0, c2.1);
            pb.line_to(d.0, d.1);
        }
    }
    if let Some(path) = pb.finish() {
        if btn == Btn::Tool(Tool::Select) {
            fill_paths.push(path.clone());
            pm.stroke_path(&path, &paint, &stroke_of(1.2 * scale), id, None);
        } else {
            pm.stroke_path(&path, &paint, &stroke_of(sw), id, None);
        }
    }
    for path in fill_paths {
        pm.fill_path(&path, &solid(Rgba { a: c.a / 3, ..c }), FillRule::Winding, id, None);
    }
}

#[derive(Clone, Copy)]
pub struct IconExtra {
    pub color: Rgba,
    pub width: f32,
}

/// Swatch in the palette popup.
pub fn swatch(pm: &mut PixmapMut, r: IRect, c: Rgba, selected: bool, scale: f32) {
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let rad = r.w as f32 / 2.0 - 3.0 * scale;
    if let Some(path) = PathBuilder::from_circle(cx, cy, rad) {
        pm.fill_path(&path, &solid(c), FillRule::Winding, Transform::identity(), None);
        pm.stroke_path(&path, &solid(Rgba { r: 255, g: 255, b: 255, a: 0x40 }), &stroke_of(1.0), Transform::identity(), None);
    }
    if selected {
        if let Some(path) = PathBuilder::from_circle(cx, cy, rad + 2.5 * scale) {
            pm.stroke_path(&path, &solid(FG), &stroke_of(1.6 * scale), Transform::identity(), None);
        }
    }
}

/// The fill toggle in the palette: a square, solid when fill is on.
pub fn fill_toggle(pm: &mut PixmapMut, r: IRect, on: bool, color: Rgba, scale: f32) {
    let inner = r.inflate(-(6.0 * scale) as i32);
    if let Some(path) = rounded_rect(inner.x as f32, inner.y as f32, inner.w as f32, inner.h as f32, 2.5 * scale) {
        if on {
            pm.fill_path(&path, &solid(color), FillRule::Winding, Transform::identity(), None);
        }
        pm.stroke_path(&path, &solid(FG), &stroke_of(1.5 * scale), Transform::identity(), None);
    }
}

/// Where the selection can be grabbed to resize it: the bracket corners and the edge grips
/// `selection_frame` draws, in the order NW, N, NE, E, SE, S, SW, W.
pub fn handle_rects(sel: IRect, scale: f32) -> [IRect; 8] {
    let c = (14.0 * scale).round() as i32;
    let (g, d) = ((26.0 * scale).round() as i32, (10.0 * scale).round() as i32);
    let (l, t, r, b) = (sel.x - 1, sel.y - 1, sel.right(), sel.bottom());
    let (mx, my) = (sel.x + sel.w / 2, sel.y + sel.h / 2);
    let at = |x: i32, y: i32, w: i32, h: i32| IRect::new(x - w / 2, y - h / 2, w, h);
    [at(l, t, c, c), at(mx, t, g, d), at(r, t, c, c), at(r, my, d, g), at(r, b, c, c), at(mx, b, g, d), at(l, b, c, c), at(l, my, d, g)]
}

/// A small dark pill with text, e.g. the selection size.
pub fn pill(pm: &mut PixmapMut, t: &GdiText, s: &str, x: i32, y: i32, scale: f32) -> IRect {
    let px = 12.0 * scale;
    let (w, h) = t.measure(s, px, true);
    let (padx, pady) = ((8.0 * scale) as i32, (3.0 * scale) as i32);
    let r = IRect::new(x, y, w + padx * 2, h + pady * 2);
    fill_rrect(pm, r.inflate(1).offset(0, 1), 6.0 * scale, Rgba { r: 0, g: 0, b: 0, a: 0x30 });
    fill_rrect(pm, r, 5.0 * scale, Rgba { r: 0x14, g: 0x14, b: 0x18, a: 0xE6 });
    stroke_rrect(pm, r, 5.0 * scale, 1.0, PANEL_BORDER);
    text(pm, t, s, px, true, x + padx, y + pady, FG);
    r
}

pub fn pill_size(t: &GdiText, s: &str, scale: f32) -> (i32, i32) {
    let (w, h) = t.measure(s, 12.0 * scale, true);
    (w + (16.0 * scale) as i32, h + (6.0 * scale) as i32)
}
