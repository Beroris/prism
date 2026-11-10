//! Drawing shapes onto a BGRA canvas. See `image.rs` for the pixel format.
//!
//! Text needs a font rasteriser, which is an OS service, so the host passes one in through
//! [`TextRaster`]. Everything else is drawn here with tiny-skia, which keeps exports identical
//! to what the overlay showed.

use crate::color::Rgba;
use crate::doc::{Shape, Style, arrow_head, counter_radius, highlighter_width, text_px};
use crate::geom::{FRect, IRect, Pt};
use crate::image::{blur_region, pixelate_region};
use tiny_skia::{BlendMode, Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, PixmapMut, Rect, Stroke, Transform};

/// An 8-bit coverage mask of rendered text, `w * h` bytes, top-down.
pub struct TextMask {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

pub trait TextRaster {
    /// Render `text` (which may contain newlines) at `px` pixels, bold if asked.
    fn raster(&self, text: &str, px: f32, bold: bool) -> Option<TextMask>;
}

/// A rasteriser that draws nothing, for tests and for builds without text.
pub struct NoText;
impl TextRaster for NoText {
    fn raster(&self, _: &str, _: f32, _: bool) -> Option<TextMask> {
        None
    }
}

/// The canvas colour for a straight RGBA colour: red and blue swapped for the BGRA buffer.
pub fn paint_color(c: Rgba) -> Color {
    Color::from_rgba8(c.b, c.g, c.r, c.a)
}

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(paint_color(c));
    p.anti_alias = true;
    p
}

fn stroke(width: f32) -> Stroke {
    Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() }
}

/// Highlighter opacity, out of 255.
pub const HIGHLIGHT_ALPHA: u8 = 110;

/// A canvas covering part of image space: pixel (0, 0) of `pixmap` is image point `origin`.
pub struct Canvas<'a> {
    pub pixmap: PixmapMut<'a>,
    pub origin: (i32, i32),
}

impl<'a> Canvas<'a> {
    pub fn new(data: &'a mut [u8], w: u32, h: u32, origin: (i32, i32)) -> Option<Canvas<'a>> {
        Some(Canvas { pixmap: PixmapMut::from_bytes(data, w, h)?, origin })
    }

    fn transform(&self) -> Transform {
        Transform::from_translate(-self.origin.0 as f32, -self.origin.1 as f32)
    }

    /// Image-space rectangle `r` in canvas pixels.
    fn local(&self, r: IRect) -> IRect {
        r.offset(-self.origin.0, -self.origin.1)
    }

    pub fn draw(&mut self, shape: &Shape, text: &dyn TextRaster) {
        let t = self.transform();
        match shape {
            Shape::Rect { a, b, style } => {
                let Some(r) = Rect::from_ltrb(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)) else { return };
                let path = PathBuilder::from_rect(r);
                if style.fill {
                    self.pixmap.fill_path(&path, &paint(style.color), FillRule::Winding, t, None);
                } else {
                    let mut s = stroke(style.width);
                    s.line_join = LineJoin::Miter;
                    self.pixmap.stroke_path(&path, &paint(style.color), &s, t, None);
                }
            }
            Shape::Ellipse { a, b, style } => {
                let Some(r) = Rect::from_ltrb(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)) else { return };
                let Some(path) = PathBuilder::from_oval(r) else { return };
                if style.fill {
                    self.pixmap.fill_path(&path, &paint(style.color), FillRule::Winding, t, None);
                } else {
                    self.pixmap.stroke_path(&path, &paint(style.color), &stroke(style.width), t, None);
                }
            }
            Shape::Line { a, b, style } => {
                let mut pb = PathBuilder::new();
                pb.move_to(a.x, a.y);
                pb.line_to(b.x, b.y);
                if let Some(path) = pb.finish() {
                    self.pixmap.stroke_path(&path, &paint(style.color), &stroke(style.width), t, None);
                }
            }
            Shape::Arrow { a, b, style } => self.arrow(*a, *b, style, t),
            Shape::Pen { pts, style } => self.polyline(pts, style.color, style.width, BlendMode::SourceOver, t),
            Shape::Highlighter { pts, style } => {
                // Translucent, not multiply: multiply vanishes on dark themes. The stroke is one
                // path, so where it crosses itself it doesn't build up.
                let c = style.color.with_alpha(HIGHLIGHT_ALPHA);
                self.polyline(pts, c, highlighter_width(style.width), BlendMode::SourceOver, t)
            }
            Shape::Highlight { rects, style } => {
                // All the boxes are one path, so where two touch they don't build up.
                let mut pb = PathBuilder::new();
                for r in rects {
                    round_rect(&mut pb, *r, (r.h() / 5.0).clamp(1.0, 4.0));
                }
                if let Some(path) = pb.finish() {
                    let c = style.color.with_alpha(HIGHLIGHT_ALPHA);
                    self.pixmap.fill_path(&path, &paint(c), FillRule::Winding, t, None);
                }
            }
            Shape::Text { pos, text: s, style, .. } => {
                if let Some(mask) = text.raster(s, text_px(style.width), false) {
                    let (x, y) = (pos.x.round() as i32 - self.origin.0, pos.y.round() as i32 - self.origin.1);
                    blend_mask(&mut self.pixmap, x, y, &mask, style.color);
                }
            }
            Shape::Counter { center, n, style } => {
                let r = counter_radius(style.width);
                if let Some(path) = PathBuilder::from_circle(center.x, center.y, r) {
                    self.pixmap.fill_path(&path, &paint(style.color), FillRule::Winding, t, None);
                    // A thin contrasting ring keeps the badge visible on any background.
                    let ring = style.color.contrasting().with_alpha(170);
                    self.pixmap.stroke_path(&path, &paint(ring), &stroke(1.5), t, None);
                }
                if let Some(mask) = text.raster(&n.to_string(), (r * 1.15).round(), true) {
                    let x = (center.x - mask.w as f32 / 2.0).round() as i32 - self.origin.0;
                    let y = (center.y - mask.h as f32 / 2.0).round() as i32 - self.origin.1;
                    blend_mask(&mut self.pixmap, x, y, &mask, style.color.contrasting());
                }
            }
            Shape::Blur { a, b, strength } => {
                let r = self.local(IRect::spanning(a.x as i32, a.y as i32, b.x as i32, b.y as i32));
                let (w, h) = (self.pixmap.width(), self.pixmap.height());
                blur_region(self.pixmap.data_mut(), w, h, r, strength.max(1.0) as u32);
            }
            Shape::Pixelate { a, b, block } => {
                let r = self.local(IRect::spanning(a.x as i32, a.y as i32, b.x as i32, b.y as i32));
                let (w, h) = (self.pixmap.width(), self.pixmap.height());
                pixelate_region(self.pixmap.data_mut(), w, h, r, block.max(2.0) as u32);
            }
        }
    }

    fn polyline(&mut self, pts: &[Pt], color: Rgba, width: f32, blend: BlendMode, t: Transform) {
        let Some(first) = pts.first() else { return };
        let mut pb = PathBuilder::new();
        pb.move_to(first.x, first.y);
        if pts.len() == 1 {
            // A click without movement still leaves a dot.
            pb.line_to(first.x + 0.01, first.y);
        } else if pts.len() == 2 {
            pb.line_to(pts[1].x, pts[1].y);
        } else {
            // Quadratic curves through the midpoints smooth out mouse jitter.
            for w in pts.windows(2).skip(1) {
                let mid = Pt::new((w[0].x + w[1].x) / 2.0, (w[0].y + w[1].y) / 2.0);
                pb.quad_to(w[0].x, w[0].y, mid.x, mid.y);
            }
            let last = pts[pts.len() - 1];
            pb.line_to(last.x, last.y);
        }
        let Some(path) = pb.finish() else { return };
        let mut p = paint(color);
        p.blend_mode = blend;
        self.pixmap.stroke_path(&path, &p, &stroke(width), t, None);
    }

    fn arrow(&mut self, a: Pt, b: Pt, style: &Style, t: Transform) {
        let len = a.dist(b);
        if len < 1.0 {
            return;
        }
        let (ux, uy) = ((b.x - a.x) / len, (b.y - a.y) / len);
        // Short arrows get a proportionally smaller head so it never swallows the shaft.
        let head = arrow_head(style.width).min(len * 0.6);
        let half = head * 0.5;
        let notch = head * 0.72;
        let base = Pt::new(b.x - ux * notch, b.y - uy * notch);
        let mut pb = PathBuilder::new();
        pb.move_to(a.x, a.y);
        pb.line_to(base.x + ux, base.y + uy);
        if let Some(shaft) = pb.finish() {
            let mut s = stroke(style.width);
            s.line_cap = LineCap::Round;
            self.pixmap.stroke_path(&shaft, &paint(style.color), &s, t, None);
        }
        let back = Pt::new(b.x - ux * head, b.y - uy * head);
        let mut pb = PathBuilder::new();
        pb.move_to(b.x, b.y);
        pb.line_to(back.x - uy * half, back.y + ux * half);
        pb.line_to(base.x, base.y);
        pb.line_to(back.x + uy * half, back.y - ux * half);
        pb.close();
        if let Some(tip) = pb.finish() {
            self.pixmap.fill_path(&tip, &paint(style.color), FillRule::Winding, t, None);
            // Round off the sharp corners slightly by stroking the outline thinly.
            let s = Stroke { width: 1.5, line_join: LineJoin::Round, ..Stroke::default() };
            self.pixmap.stroke_path(&tip, &paint(style.color), &s, t, None);
        }
    }
}

/// Add a rounded rectangle to `pb`, clockwise like every other one so they join up.
fn round_rect(pb: &mut PathBuilder, r: FRect, rad: f32) {
    if r.w() <= 0.0 || r.h() <= 0.0 {
        return;
    }
    let k = rad.min(r.w() / 2.0).min(r.h() / 2.0);
    // Control points for a quarter circle drawn as one cubic.
    let c = k * 0.4477;
    pb.move_to(r.l + k, r.t);
    pb.line_to(r.r - k, r.t);
    pb.cubic_to(r.r - c, r.t, r.r, r.t + c, r.r, r.t + k);
    pb.line_to(r.r, r.b - k);
    pb.cubic_to(r.r, r.b - c, r.r - c, r.b, r.r - k, r.b);
    pb.line_to(r.l + k, r.b);
    pb.cubic_to(r.l + c, r.b, r.l, r.b - c, r.l, r.b - k);
    pb.line_to(r.l, r.t + k);
    pb.cubic_to(r.l, r.t + c, r.l + c, r.t, r.l + k, r.t);
    pb.close();
}

/// Composite `color` through an 8-bit coverage mask with its top-left at (x, y).
pub fn blend_mask(pm: &mut PixmapMut, x: i32, y: i32, mask: &TextMask, color: Rgba) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let src = [color.b as u32, color.g as u32, color.r as u32];
    for my in 0..mask.h as i32 {
        let py = y + my;
        if py < 0 || py >= h {
            continue;
        }
        for mx in 0..mask.w as i32 {
            let px = x + mx;
            if px < 0 || px >= w {
                continue;
            }
            let cov = mask.data[(my as u32 * mask.w + mx as u32) as usize] as u32;
            let a = cov * color.a as u32 / 255;
            if a == 0 {
                continue;
            }
            let i = ((py * w + px) * 4) as usize;
            let inv = 255 - a;
            for c in 0..3 {
                data[i + c] = ((src[c] * a + data[i + c] as u32 * inv + 127) / 255) as u8;
            }
            data[i + 3] = (a + data[i + 3] as u32 * inv / 255).min(255) as u8;
        }
    }
}

/// Render `shapes` over the pixels already in `canvas`, in order. Blur and pixelate act on
/// whatever is below them, so their place in the list matters.
pub fn draw_all(canvas: &mut Canvas, shapes: &[Shape], text: &dyn TextRaster) {
    for s in shapes {
        canvas.draw(s, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Style;
    use crate::image::Image;

    fn white(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        img.data.fill(255);
        img
    }

    #[test]
    fn rect_outline_paints_edge_in_bgra_order() {
        let mut img = white(50, 50);
        let shape = Shape::Rect { a: Pt::new(10.0, 10.0), b: Pt::new(40.0, 40.0), style: Style { width: 4.0, ..Style::default() } };
        let mut c = Canvas::new(&mut img.data, 50, 50, (0, 0)).unwrap();
        c.draw(&shape, &NoText);
        let (r, g, b) = img.rgb_at(10, 25).unwrap();
        assert_eq!((r, g, b), (Rgba::RED.r, Rgba::RED.g, Rgba::RED.b));
        assert_eq!(img.rgb_at(25, 25), Some((255, 255, 255)));
    }

    #[test]
    fn origin_offsets_drawing() {
        let mut img = white(20, 20);
        let shape = Shape::Rect { a: Pt::new(100.0, 100.0), b: Pt::new(110.0, 110.0), style: Style { fill: true, ..Style::default() } };
        let mut c = Canvas::new(&mut img.data, 20, 20, (95, 95)).unwrap();
        c.draw(&shape, &NoText);
        assert_eq!(img.rgb_at(10, 10), Some((Rgba::RED.r, Rgba::RED.g, Rgba::RED.b)));
        assert_eq!(img.rgb_at(2, 2), Some((255, 255, 255)));
    }

    #[test]
    fn highlighter_is_translucent_on_light_and_dark() {
        let mut img = white(40, 40);
        // A black pixel under the stroke must stay dark (text readable) but visibly tinted.
        let i = (20 * 40 + 20) * 4;
        img.data[i..i + 3].fill(0);
        let shape = Shape::Highlighter { pts: vec![Pt::new(5.0, 20.0), Pt::new(35.0, 20.0)], style: Style { color: Rgba::YELLOW, width: 4.0, fill: false } };
        let mut c = Canvas::new(&mut img.data, 40, 40, (0, 0)).unwrap();
        c.draw(&shape, &NoText);
        let (r, g, b) = img.rgb_at(20, 20).unwrap();
        assert!(r > 60 && r < 160 && b < 40, "tinted but dark: {r},{g},{b}");
        let (r, g, b) = img.rgb_at(10, 20).unwrap();
        assert!(r == 255 && g > 220 && b < 200, "yellow on white: {r},{g},{b}");
    }

    #[test]
    fn touching_highlight_boxes_do_not_build_up() {
        let mut img = white(40, 40);
        let style = Style { color: Rgba::YELLOW, width: 4.0, fill: false };
        let rects = vec![FRect { l: 5.0, t: 5.0, r: 30.0, b: 20.0 }, FRect { l: 5.0, t: 15.0, r: 30.0, b: 35.0 }];
        let mut c = Canvas::new(&mut img.data, 40, 40, (0, 0)).unwrap();
        c.draw(&Shape::Highlight { rects, style }, &NoText);
        assert_eq!(img.rgb_at(15, 10), img.rgb_at(15, 17), "the overlap is no darker");
        assert_ne!(img.rgb_at(15, 10), Some((255, 255, 255)));
        assert_eq!(img.rgb_at(35, 10), Some((255, 255, 255)));
    }

    #[test]
    fn blend_mask_full_and_clipped() {
        let mut img = white(4, 4);
        let mask = TextMask { w: 3, h: 3, data: vec![255; 9] };
        let mut pm = PixmapMut::from_bytes(&mut img.data, 4, 4).unwrap();
        blend_mask(&mut pm, 2, 2, &mask, Rgba::BLUE);
        assert_eq!(img.rgb_at(3, 3), Some((Rgba::BLUE.r, Rgba::BLUE.g, Rgba::BLUE.b)));
        assert_eq!(img.rgb_at(1, 1), Some((255, 255, 255)));
    }

    #[test]
    fn arrow_head_reaches_tip() {
        let mut img = white(100, 40);
        let shape = Shape::Arrow { a: Pt::new(5.0, 20.0), b: Pt::new(90.0, 20.0), style: Style::default() };
        let mut c = Canvas::new(&mut img.data, 100, 40, (0, 0)).unwrap();
        c.draw(&shape, &NoText);
        assert_ne!(img.rgb_at(88, 20), Some((255, 255, 255)));
        assert_eq!(img.rgb_at(95, 20), Some((255, 255, 255)));
    }
}
