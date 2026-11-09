//! The app icon, drawn in code so every size is crisp: a flat, full-bleed violet rounded square
//! with a bold white camera aperture. Geometry is in a 64-unit box.
//! Used for the tray at runtime and to generate the `.ico` files (see `examples/make_ico.rs`).

use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

/// Tile colour: Tailwind violet-600 (#7c3aed).
const TILE: (u8, u8, u8) = (0x7C, 0x3A, 0xED);

/// Aperture blades in a 24-unit box centred on (12, 12) with the rim at radius 10: each is a
/// line from an inner hexagon corner out to the rim.
const BLADES: [(f32, f32, f32, f32); 6] = [
    (14.31, 8.0, 20.05, 17.94),
    (9.69, 8.0, 21.17, 8.0),
    (7.38, 12.0, 13.12, 2.06),
    (9.69, 16.0, 3.95, 6.06),
    (14.31, 16.0, 2.83, 16.0),
    (16.62, 12.0, 10.88, 21.94),
];

/// Draw the icon at `size`×`size` as **RGBA** premultiplied (tiny-skia's native order).
pub fn render_rgba(size: u32) -> Pixmap {
    render_turned(size, 0.0)
}

/// The icon with its aperture blades turned by `turn` radians, as a shutter turns: the welcome
/// animates it.
pub fn render_turned(size: u32, turn: f32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("icon size");
    let s = size as f32 / 64.0;
    let c = size as f32 / 2.0;

    let mut tile = Paint::default();
    tile.anti_alias = true;
    tile.set_color_rgba8(TILE.0, TILE.1, TILE.2, 255);
    if let Some(p) = rounded_rect(0.0, 0.0, size as f32, size as f32, 14.0 * s) {
        pm.fill_path(&p, &tile, FillRule::Winding, Transform::identity(), None);
    }

    let mut white = Paint::default();
    white.anti_alias = true;
    white.set_color_rgba8(255, 255, 255, 255);
    // Ring: outer edge at radius 21.2, with a light stroke so the blades have room.
    let rim = 18.6;
    let mut pb = PathBuilder::new();
    pb.push_circle(c, c, rim * s);
    if let Some(p) = pb.finish() {
        let stroke = Stroke { width: (5.2 * s).max(1.6), ..Stroke::default() };
        pm.stroke_path(&p, &white, &stroke, Transform::identity(), None);
    }
    // Blades, scaled so their outer ends meet the ring's centre line.
    let k = rim / 10.0 * s;
    let (sin, cos) = turn.sin_cos();
    let at = |x: f32, y: f32| {
        let (dx, dy) = ((x - 12.0) * k, (y - 12.0) * k);
        (c + dx * cos - dy * sin, c + dx * sin + dy * cos)
    };
    let mut pb = PathBuilder::new();
    for (x1, y1, x2, y2) in BLADES {
        let (ax, ay) = at(x1, y1);
        let (bx, by) = at(x2, y2);
        pb.move_to(ax, ay);
        pb.line_to(bx, by);
    }
    if let Some(p) = pb.finish() {
        let stroke = Stroke { width: (3.4 * s).max(1.1), line_cap: LineCap::Round, ..Stroke::default() };
        pm.stroke_path(&p, &white, &stroke, Transform::identity(), None);
    }
    pm
}

/// Same icon as BGRA premultiplied, for Windows icon bitmaps.
pub fn render_bgra(size: u32) -> Vec<u8> {
    let mut data = render_rgba(size).take();
    data.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
    data
}

pub fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= 0.0 {
        return Some(PathBuilder::from_rect(Rect::from_xywh(x, y, w, h)?));
    }
    // Cubic approximation of quarter circles.
    let k = r * 0.552_284_8;
    let (l, t, rr, b) = (x, y, x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(l + r, t);
    pb.line_to(rr - r, t);
    pb.cubic_to(rr - r + k, t, rr, t + r - k, rr, t + r);
    pb.line_to(rr, b - r);
    pb.cubic_to(rr, b - r + k, rr - r + k, b, rr - r, b);
    pb.line_to(l + r, b);
    pb.cubic_to(l + r - k, b, l, b - r + k, l, b - r);
    pb.line_to(l, t + r);
    pb.cubic_to(l, t + r - k, l + r - k, t, l + r, t);
    pb.close();
    pb.finish()
}

/// Build a `.ico` file with PNG-compressed entries at the given sizes.
pub fn build_ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&sz| {
            let pm = render_rgba(sz);
            // ICO wants straight alpha; tiny-skia holds premultiplied.
            let mut straight = pm.take_demultiplied();
            let mut out = Vec::new();
            {
                let mut enc = png::Encoder::new(&mut out, sz, sz);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                let mut w = enc.write_header().expect("png header");
                w.write_image_data(&straight).expect("png data");
            }
            straight.clear();
            out
        })
        .collect();
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (sz, img) in sizes.iter().zip(&images) {
        let dim = if *sz >= 256 { 0 } else { *sz as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(img.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        ico.extend_from_slice(&img);
    }
    ico
}
