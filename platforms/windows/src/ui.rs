//! Drawing the overlay's chrome: panels, text, toolbar icons. Everything is vector-drawn at the
//! monitor's scale, so it is sharp at any DPI and needs no image assets.
//!
//! Colours are given as straight RGBA and converted for the BGRA canvas by `paint_color`.

use crate::text::GdiText;
use prism_core::doc::Tool;
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

/// Selection outline: a crisp line with a dark halo so it shows on any background.
pub fn selection_border(pm: &mut PixmapMut, r: IRect, scale: f32) {
    let w = (scale).max(1.0).round();
    let outer = r.inflate(w as i32);
    stroke_rect_px(pm, outer.inflate(1), 1.0, Rgba { r: 0, g: 0, b: 0, a: 0x50 });
    stroke_rect_px(pm, outer, w, ACCENT);
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

pub struct ToolbarLayout {
    pub panel: IRect,
    pub buttons: Vec<(Btn, IRect)>,
    pub separators: Vec<IRect>,
    /// Whether the toolbar sits above the selection (tooltips and the palette then open upward).
    pub above: bool,
}

pub fn layout_toolbar(sel: IRect, monitor: IRect, scale: f32) -> ToolbarLayout {
    let b = (32.0 * scale).round() as i32;
    let pad = (4.0 * scale).round() as i32;
    let sep = (9.0 * scale).round() as i32;
    let gap = (8.0 * scale).round() as i32;
    let items = toolbar_items();
    let width = pad * 2 + items.iter().map(|i| if i.is_some() { b } else { sep }).sum::<i32>();
    let height = b + pad * 2;
    // Prefer below, right-aligned with the selection; then above; then inside the bottom edge.
    let x = (sel.right() - width).max(monitor.x + gap).min(monitor.right() - width - gap);
    let (y, above) = if sel.bottom() + gap + height <= monitor.bottom() - gap {
        (sel.bottom() + gap, false)
    } else if sel.y - gap - height >= monitor.y + gap {
        (sel.y - gap - height, true)
    } else {
        (sel.bottom() - gap - height, true)
    };
    let panel = IRect::new(x.max(monitor.x), y, width, height);
    let mut buttons = Vec::new();
    let mut separators = Vec::new();
    let mut cx = panel.x + pad;
    for item in items {
        match item {
            Some(btn) => {
                buttons.push((btn, IRect::new(cx, panel.y + pad, b, b)));
                cx += b;
            }
            None => {
                separators.push(IRect::new(cx + sep / 2, panel.y + pad + b / 4, 1, b / 2));
                cx += sep;
            }
        }
    }
    ToolbarLayout { panel, buttons, separators, above }
}

/// Palette popup: one swatch per colour, then a fill toggle.
pub struct PaletteLayout {
    pub panel: IRect,
    pub swatches: Vec<(Rgba, IRect)>,
    pub fill: IRect,
}

pub fn layout_palette(anchor: IRect, above: bool, colors: &[Rgba], monitor: IRect, scale: f32) -> PaletteLayout {
    let s = (26.0 * scale).round() as i32;
    let pad = (6.0 * scale).round() as i32;
    let gap = (4.0 * scale).round() as i32;
    let n = colors.len() as i32 + 1;
    let width = pad * 2 + n * s + (n - 1) * gap + gap * 2;
    let height = s + pad * 2;
    let x = (anchor.x + anchor.w / 2 - width / 2).clamp(monitor.x + 4, (monitor.right() - width - 4).max(monitor.x + 4));
    let y = if above { anchor.y - height - (10.0 * scale) as i32 } else { anchor.bottom() + (10.0 * scale) as i32 };
    let panel = IRect::new(x, y, width, height);
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

/// Selection resize handles: corners and edge midpoints.
pub fn handle_rects(sel: IRect, scale: f32) -> [IRect; 8] {
    let s = (7.0 * scale).round() as i32 | 1;
    let h = s / 2;
    let (l, t, r, b) = (sel.x - 1, sel.y - 1, sel.right(), sel.bottom());
    let (mx, my) = (sel.x + sel.w / 2, sel.y + sel.h / 2);
    let at = |x: i32, y: i32| IRect::new(x - h, y - h, s, s);
    [at(l, t), at(mx, t), at(r, t), at(r, my), at(r, b), at(mx, b), at(l, b), at(l, my)]
}

pub fn draw_handles(pm: &mut PixmapMut, sel: IRect, scale: f32) {
    for h in handle_rects(sel, scale) {
        fill_rrect(pm, h.inflate(1), 2.0, Rgba { r: 0, g: 0, b: 0, a: 0x60 });
        fill_rrect(pm, h, 1.5 * scale, Rgba::WHITE);
        stroke_rrect(pm, h, 1.5 * scale, 1.0, ACCENT);
    }
}

/// A small dark pill with text, e.g. the selection size.
pub fn pill(pm: &mut PixmapMut, t: &GdiText, s: &str, x: i32, y: i32, scale: f32) -> IRect {
    let px = 12.0 * scale;
    let (w, h) = t.measure(s, px, true);
    let (padx, pady) = ((8.0 * scale) as i32, (3.0 * scale) as i32);
    let r = IRect::new(x, y, w + padx * 2, h + pady * 2);
    fill_rrect(pm, r, 5.0 * scale, Rgba { r: 0x14, g: 0x14, b: 0x18, a: 0xE6 });
    text(pm, t, s, px, true, x + padx, y + pady, FG);
    r
}

pub fn pill_size(t: &GdiText, s: &str, scale: f32) -> (i32, i32) {
    let (w, h) = t.measure(s, 12.0 * scale, true);
    (w + (16.0 * scale) as i32, h + (6.0 * scale) as i32)
}
